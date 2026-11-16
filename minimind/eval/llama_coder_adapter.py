import argparse
import hashlib
import json
import re
import sys
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coder_server import parse_response


def render_messages(payload):
    rendered = []
    tools = payload.get("tools") or []
    if tools:
        rendered.append({
            "role": "system",
            "content": (
                "你是 PocketMind。所有项目操作必须通过工具；写文件必须等待用户批准。"
                "只输出工具调用 JSON 数组，不要输出代码或解释。工具定义："
                + json.dumps(tools, ensure_ascii=False)
            ),
        })

    workspace = payload.get("workspace") or {}
    workspace_files = workspace.get("files") or []
    if workspace_files:
        paths = [
            str(item.get("path", ""))
            for item in workspace_files
            if isinstance(item, dict) and item.get("path")
        ]
        excerpts = []
        excerpt_bytes = 0
        for item in workspace_files:
            if not isinstance(item, dict) or not isinstance(item.get("content"), str):
                continue
            value = f"\n--- {item.get('path', '')} ---\n{item['content']}"
            encoded_size = len(value.encode("utf-8"))
            if excerpt_bytes + encoded_size > 24_000:
                break
            excerpts.append(value)
            excerpt_bytes += encoded_size
        context = "项目文件索引（只能引用这些相对路径）：\n" + "\n".join(paths[:240])
        if excerpts:
            context += "\n\n已选择文件内容：" + "".join(excerpts)
        rendered.append({"role": "system", "content": context})

    for message in payload.get("messages") or []:
        role = message.get("role", "user")
        content = message.get("content", "")
        if message.get("tool_calls") is not None:
            content = json.dumps(message["tool_calls"], ensure_ascii=False)
        if role == "tool":
            rendered.append({"role": "user", "content": f"Tool result: {content}"})
        else:
            rendered.append({"role": role, "content": content})
    return rendered


def test_paths(payload):
    text = " ".join(str(message.get("content", "")) for message in payload.get("messages") or [])
    return sorted(set(re.findall(r"(?:test_[A-Za-z0-9_.-]+|[A-Za-z0-9_.-]*test[A-Za-z0-9_.-]*)\.py", text)))


def normalize_result(payload, text):
    parsed = parse_response(text, test_paths(payload))
    requested = payload.get("requested_tool") or {}
    requested_arguments = requested.get("arguments") or {}
    if (
        payload.get("mode") == "write_content"
        and parsed.get("type") == "final"
        and requested.get("name") == "write_file"
        and isinstance(requested_arguments.get("path"), str)
    ):
        content = re.sub(r"^Tool result:\s*", "", text.strip()) + "\n"
        parsed = {
            "type": "tool_request",
            "content": "",
            "tool": {
                "name": "write_file",
                "arguments": {"path": requested_arguments["path"], "content": content},
            },
        }
    return parsed


class Runtime:
    def __init__(self, upstream_host, upstream_port, model_path):
        self.upstream_host = upstream_host
        self.upstream_port = upstream_port
        self.runtime = "llama.cpp-q4_k_m"
        self.status = "ready"
        self.digest = hashlib.sha256(Path(model_path).read_bytes()).hexdigest()

    def _request(self, payload, stream):
        body = json.dumps({
            "model": "qwen2.5-coder-1.5b-q4_k_m",
            "messages": render_messages(payload),
            "temperature": 0,
            # 256 frequently truncates a complete code example immediately
            # before the requested explanation. 384 remains bounded for the
            # portable CPU model while allowing short code + analysis turns.
            "max_tokens": 384,
            "stream": stream,
        }, ensure_ascii=False).encode("utf-8")
        connection = HTTPConnection(self.upstream_host, self.upstream_port, timeout=300)
        connection.request(
            "POST",
            "/v1/chat/completions",
            body=body,
            headers={"content-type": "application/json", "content-length": str(len(body))},
        )
        response = connection.getresponse()
        if response.status != 200:
            detail = response.read(4096).decode("utf-8", errors="replace")
            connection.close()
            raise RuntimeError(f"llama.cpp returned HTTP {response.status}: {detail}")
        return connection, response

    def generate(self, payload):
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            return {"type": "error", "content": "messages must be a non-empty list", "tool": None}
        connection, response = self._request(payload, False)
        try:
            value = json.loads(response.read())
            text = value["choices"][0]["message"].get("content") or ""
            return normalize_result(payload, text)
        finally:
            connection.close()

    def generate_stream(self, payload):
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            raise ValueError("messages must be a non-empty list")
        if payload.get("tools"):
            result = self.generate(payload)
            if result.get("content"):
                yield {"type": "text_delta", "delta": result["content"]}
            yield {"type": "complete", "result": result}
            return

        connection, response = self._request(payload, True)
        parts = []
        try:
            while True:
                line = response.readline()
                if not line:
                    break
                decoded = line.decode("utf-8", errors="replace").strip()
                if not decoded.startswith("data:"):
                    continue
                data = decoded[5:].strip()
                if data == "[DONE]":
                    break
                event = json.loads(data)
                delta = event.get("choices", [{}])[0].get("delta", {}).get("content") or ""
                if delta:
                    parts.append(delta)
                    yield {"type": "text_delta", "delta": delta}
        finally:
            connection.close()
        text = "".join(parts)
        yield {"type": "complete", "result": normalize_result(payload, text)}


class Handler(BaseHTTPRequestHandler):
    runtime = None

    def send_json(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            return self.send_json(200, {
                "status": self.runtime.status,
                "runtime": self.runtime.runtime,
                "model_sha256": self.runtime.digest,
                "upstream": f"{self.runtime.upstream_host}:{self.runtime.upstream_port}",
            })
        self.send_json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        if self.path not in {"/generate", "/generate/stream"}:
            return self.send_json(404, {"error": {"message": "not found"}})
        try:
            length = int(self.headers.get("content-length", "0"))
            if length > 1_048_576:
                return self.send_json(413, {"error": {"message": "body too large"}})
            payload = json.loads(self.rfile.read(length))
            if self.path == "/generate":
                return self.send_json(200, self.runtime.generate(payload))
            self.send_response(200)
            self.send_header("content-type", "application/x-ndjson; charset=utf-8")
            self.send_header("cache-control", "no-store")
            self.end_headers()
            try:
                for event in self.runtime.generate_stream(payload):
                    self.wfile.write((json.dumps(event, ensure_ascii=False) + "\n").encode("utf-8"))
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                return
            except Exception as error:
                event = {"type": "error", "error": {"message": type(error).__name__, "detail": str(error)}}
                try:
                    self.wfile.write((json.dumps(event, ensure_ascii=False) + "\n").encode("utf-8"))
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass
        except Exception as error:
            self.send_json(500, {"error": {"message": type(error).__name__, "detail": str(error)}})

    def log_message(self, format, *args):
        return


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--upstream-host", default="127.0.0.1")
    parser.add_argument("--upstream-port", type=int, default=3285)
    parser.add_argument("--port", type=int, default=3286)
    args = parser.parse_args()
    Handler.runtime = Runtime(args.upstream_host, args.upstream_port, args.model)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
