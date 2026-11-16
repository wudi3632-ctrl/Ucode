import argparse
import hashlib
import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def parse_response(text, default_test_paths=None):
    default_test_paths = list(default_test_paths or [])

    def normalize(call):
        if not isinstance(call, dict):
            return None
        if isinstance(call.get("function"), dict):
            call = call["function"]
        name = call.get("name") or call.get("type")
        if not isinstance(name, str):
            return None
        if name in {"test", "test_request", "run_tests"}:
            name = "pytest"
        arguments = call.get("arguments") or call.get("parameters") or {}
        if not isinstance(arguments, dict):
            arguments = {}
        if isinstance(arguments, dict) and "filename" in arguments and "path" not in arguments:
            arguments = dict(arguments)
            arguments["path"] = arguments.pop("filename")
        if name == "pytest" and not arguments.get("paths"):
            arguments = dict(arguments)
            arguments["paths"] = default_test_paths
        return {"type": "tool_request", "content": "", "tool": {"name": name, "arguments": arguments}}

    for value in re.findall(r"<tool_call>\s*(.*?)\s*</tool_call>", text, re.DOTALL):
        try:
            call = json.loads(value)
        except json.JSONDecodeError:
            continue
        result = normalize(call)
        if result:
            return result
    start = text.find("[")
    if start >= 0:
        try:
            values, _ = json.JSONDecoder().raw_decode(text[start:])
            calls = values if isinstance(values, list) else [values]
            call = next((item for item in calls if isinstance(item, dict) and ((item.get("name") or item.get("type")) == "write_file" or item.get("function", {}).get("name") == "write_file")), calls[0])
            result = normalize(call)
            if result:
                return result
        except (json.JSONDecodeError, IndexError, TypeError):
            pass
    try:
        structured = json.loads(text)
        if isinstance(structured, dict) and isinstance(structured.get("tool"), dict):
            result = normalize(structured["tool"])
            if result:
                return result
        if isinstance(structured, dict) and structured.get("type") in {"test", "test_request", "run_tests"}:
            result = normalize(structured)
            if result:
                return result
    except json.JSONDecodeError:
        pass
    if text.lstrip().startswith("{") and "pytest" in text and re.search(r'"paths"\s*:', text):
        values = re.findall(r'"paths"\s*:\s*\[(.*?)\]', text, re.DOTALL)
        paths = re.findall(r'"([^"]+)"', values[0]) if values else []
        return {"type": "tool_request", "content": "", "tool": {"name": "pytest", "arguments": {"paths": paths}}}
    for name in ("write_file", "pytest", "read_file"):
        if re.search(rf'"name"\s*:\s*"{name}"', text):
            path = re.search(r'"(?:path|filename)"\s*:\s*"([^"]+)"', text)
            content = re.search(r'"content"\s*:\s*"(.*?)"', text, re.DOTALL)
            arguments = {}
            if path:
                arguments["path"] = path.group(1)
            if content:
                arguments["content"] = content.group(1).replace("\\n", "\n")
            if name == "pytest":
                arguments = {"paths": [path.group(1)]} if path else {"paths": []}
            return {"type": "tool_request", "content": "", "tool": {"name": name, "arguments": arguments}}
    return {"type": "final", "content": text, "tool": None}


class Runtime:
    def __init__(self, model_path, checkpoint, device, quantize_int8=False):
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        self.device = device
        self.model_path = model_path
        self.checkpoint_digest = hashlib.sha256(Path(checkpoint).read_bytes()).hexdigest() if checkpoint else None
        self.tokenizer = AutoTokenizer.from_pretrained(model_path, local_files_only=True)
        dtype = torch.bfloat16 if device.startswith("cuda") else torch.float32
        self.model = AutoModelForCausalLM.from_pretrained(model_path, local_files_only=True, dtype=dtype).to(device).eval()
        if checkpoint:
            state = torch.load(checkpoint, map_location=device, weights_only=True)
            self.model.load_state_dict(state["model"], strict=True)
        self.runtime = "pytorch-fp32"
        if quantize_int8:
            if device != "cpu":
                raise ValueError("dynamic INT8 quantization is only available on CPU")
            self.model = torch.ao.quantization.quantize_dynamic(
                self.model, {torch.nn.Linear}, dtype=torch.qint8, inplace=False
            ).eval()
            self.runtime = "pytorch-int8"
        self.status = "ready"
        self.digest = hashlib.sha256((Path(model_path) / "model.safetensors").read_bytes()).hexdigest()

    def generate(self, payload):
        import torch
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            return {"type": "error", "content": "messages must be a non-empty list", "tool": None}
        if messages[-1].get("role") == "tool" and self._pytest_succeeded(messages[-1].get("content", "")):
            return {"type": "final", "content": "测试通过；修改已完成。", "tool": None}
        rendered = []
        tools = payload.get("tools") or []
        if tools:
            rendered.append({"role": "system", "content": "你是 PocketMind。所有项目操作必须通过工具；写文件必须等待用户批准。只输出工具调用 JSON 数组，不要输出 Python 代码或解释。工具定义：" + json.dumps(tools, ensure_ascii=False)})
        workspace = payload.get("workspace") or {}
        workspace_files = workspace.get("files") or []
        if workspace_files:
            paths = [str(item.get("path", "")) for item in workspace_files if isinstance(item, dict) and item.get("path")]
            excerpts = []
            excerpt_bytes = 0
            for item in workspace_files:
                if not isinstance(item, dict) or not isinstance(item.get("content"), str):
                    continue
                value = f"\n--- {item.get('path', '')} ---\n{item['content']}"
                encoded = value.encode("utf-8")
                if excerpt_bytes + len(encoded) > 24_000:
                    break
                excerpts.append(value)
                excerpt_bytes += len(encoded)
            context = "项目文件索引（只能引用这些相对路径）：\n" + "\n".join(paths[:240])
            if excerpts:
                context += "\n\n已选择文件内容：" + "".join(excerpts)
            rendered.append({"role": "system", "content": context})
        for message in messages:
            role = message.get("role", "user")
            content = message.get("content", "")
            if message.get("tool_calls") is not None:
                content = json.dumps(message["tool_calls"], ensure_ascii=False)
            if role == "tool":
                rendered.append({"role": "user", "content": f"Tool result: {content}"})
            else:
                rendered.append({"role": role, "content": content})
        try:
            prompt = self.tokenizer.apply_chat_template(rendered, tokenize=False, add_generation_prompt=True)
        except Exception:
            prompt = "\n".join(f"{m['role']}: {m['content']}" for m in rendered) + "\nassistant:"
        inputs = self.tokenizer(prompt, return_tensors="pt", truncation=True, max_length=2048).to(self.device)
        with torch.inference_mode():
            output = self.model.generate(**inputs, max_new_tokens=256, do_sample=False, pad_token_id=self.tokenizer.eos_token_id)
        text = self.tokenizer.decode(output[0, inputs.input_ids.shape[1]:], skip_special_tokens=True)
        print(json.dumps({"event": "coder.raw", "text": text[:2000]}, ensure_ascii=False), flush=True)
        test_paths = sorted(set(re.findall(r"(?:test_[A-Za-z0-9_.-]+|[A-Za-z0-9_.-]*test[A-Za-z0-9_.-]*)\.py", " ".join(str(message.get("content", "")) for message in messages))))
        parsed = parse_response(text, test_paths)
        requested = payload.get("requested_tool") or {}
        requested_arguments = requested.get("arguments") or {}
        if payload.get("mode") == "write_content" and parsed.get("type") == "final" and requested.get("name") == "write_file" and isinstance(requested_arguments.get("path"), str):
            content = re.sub(r"^Tool result:\s*", "", text.strip()) + "\n"
            parsed = {"type": "tool_request", "content": "", "tool": {"name": "write_file", "arguments": {"path": requested_arguments["path"], "content": content}}}
        print(json.dumps({"event": "coder.parsed", "parsed": parsed}, ensure_ascii=False), flush=True)
        return parsed

    def generate_stream(self, payload, cancelled):
        """Yield decoded model tokens and a final structured response as NDJSON-ready objects."""
        import torch
        from transformers import StoppingCriteria, StoppingCriteriaList, TextIteratorStreamer

        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            raise ValueError("messages must be a non-empty list")
        if payload.get("tools"):
            result = self.generate(payload)
            if result.get("content"):
                yield {"type": "text_delta", "delta": result["content"]}
            yield {"type": "complete", "result": result}
            return

        rendered = []
        workspace = payload.get("workspace") or {}
        workspace_files = workspace.get("files") or []
        if workspace_files:
            paths = [str(item.get("path", "")) for item in workspace_files if isinstance(item, dict) and item.get("path")]
            excerpts = []
            excerpt_bytes = 0
            for item in workspace_files:
                if not isinstance(item, dict) or not isinstance(item.get("content"), str):
                    continue
                value = f"\n--- {item.get('path', '')} ---\n{item['content']}"
                if excerpt_bytes + len(value.encode("utf-8")) > 24_000:
                    break
                excerpts.append(value)
                excerpt_bytes += len(value.encode("utf-8"))
            context = "项目文件索引（只能引用这些相对路径）：\n" + "\n".join(paths[:240])
            if excerpts:
                context += "\n\n已选择文件内容：" + "".join(excerpts)
            rendered.append({"role": "system", "content": context})
        for message in messages:
            role = message.get("role", "user")
            content = message.get("content", "")
            if role == "tool":
                rendered.append({"role": "user", "content": f"Tool result: {content}"})
            else:
                rendered.append({"role": role, "content": content})
        try:
            prompt = self.tokenizer.apply_chat_template(rendered, tokenize=False, add_generation_prompt=True)
        except Exception:
            prompt = "\n".join(f"{item['role']}: {item['content']}" for item in rendered) + "\nassistant:"
        inputs = self.tokenizer(prompt, return_tensors="pt", truncation=True, max_length=2048).to(self.device)
        streamer = TextIteratorStreamer(self.tokenizer, skip_prompt=True, skip_special_tokens=True, timeout=1.0)

        class StopWhenCancelled(StoppingCriteria):
            def __call__(self, input_ids, scores, **kwargs):
                return cancelled.is_set()

        failure = []
        def run_generation():
            try:
                with torch.inference_mode():
                    self.model.generate(
                        **inputs, max_new_tokens=256, do_sample=False,
                        pad_token_id=self.tokenizer.eos_token_id, streamer=streamer,
                        stopping_criteria=StoppingCriteriaList([StopWhenCancelled()]),
                    )
            except Exception as error:
                failure.append(error)
                streamer.end()

        worker = threading.Thread(target=run_generation, daemon=True)
        worker.start()
        parts = []
        while worker.is_alive() or not cancelled.is_set():
            try:
                piece = next(streamer)
            except StopIteration:
                break
            except Exception:
                if worker.is_alive():
                    continue
                break
            if piece:
                parts.append(piece)
                yield {"type": "text_delta", "delta": piece}
            if cancelled.is_set():
                break
        worker.join(timeout=2)
        if cancelled.is_set():
            yield {"type": "cancelled"}
            return
        if failure:
            raise failure[0]
        text = "".join(parts)
        yield {"type": "complete", "result": parse_response(text)}

    @staticmethod
    def _pytest_succeeded(content):
        try:
            value = json.loads(content) if isinstance(content, str) else content
        except json.JSONDecodeError:
            return False

        def walk(item):
            if isinstance(item, dict):
                if "exitCode" in item:
                    return item["exitCode"] == 0
                if item.get("status") in {"passed", "success", "succeeded"}:
                    return True
                return any(walk(child) for child in item.values())
            if isinstance(item, list):
                return any(walk(child) for child in item)
            return False

        return walk(value)


class Handler(BaseHTTPRequestHandler):
    runtime = None

    def send_json(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            return self.send_json(200, {"status": self.runtime.status, "runtime": self.runtime.runtime, "model_sha256": self.runtime.digest, "checkpoint_sha256": self.runtime.checkpoint_digest})
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
            cancelled = threading.Event()
            self.send_response(200)
            self.send_header("content-type", "application/x-ndjson; charset=utf-8")
            self.send_header("cache-control", "no-store")
            self.end_headers()
            try:
                for event in self.runtime.generate_stream(payload, cancelled):
                    self.wfile.write((json.dumps(event, ensure_ascii=False) + "\n").encode("utf-8"))
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                cancelled.set()
        except Exception as error:
            self.send_json(500, {"error": {"message": type(error).__name__}})

    def log_message(self, format, *args):
        return


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path)
    parser.add_argument("--device", default="cuda")
    parser.add_argument("--port", type=int, default=3283)
    parser.add_argument("--quantize-int8", action="store_true")
    args = parser.parse_args()
    Handler.runtime = Runtime(args.model, args.checkpoint, args.device, args.quantize_int8)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
