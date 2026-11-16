import argparse
import hashlib
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VENDOR = ROOT / "vendor" / "minimind"
sys.path.insert(0, str(VENDOR))
ALLOWED_TOOLS = {"read_file", "search", "write_file", "diff", "pytest"}


def sha256_file(path: Path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def startup_stage(stage, **details):
    print(json.dumps({"event": "model.startup", "stage": stage, **details}), flush=True)


def tool_response(value):
    if not isinstance(value, dict) or value.get("name") not in ALLOWED_TOOLS or not isinstance(value.get("arguments", {}), dict):
        return None
    return {"type": "tool_request", "content": "", "tool": {"name": value["name"], "arguments": value["arguments"]}}


def parse_response(text):
    try:
        structured = json.loads(text)
        if structured.get("type") in {"message", "tool_request", "final", "error"}:
            return structured
        result = tool_response(structured)
        if result:
            return result
    except (json.JSONDecodeError, AttributeError):
        pass
    for value in re.findall(r"<tool_call>\s*(.*?)\s*</tool_call>", text, re.DOTALL):
        try:
            call = json.loads(value)
        except json.JSONDecodeError:
            continue
        result = tool_response(call)
        if result:
            return result
    stripped = text.lstrip()
    if stripped.startswith("{"):
        try:
            value, _ = json.JSONDecoder().raw_decode(stripped)
            result = tool_response(value)
            if result:
                return result
        except json.JSONDecodeError:
            pass
    return {"type": "message", "content": text, "tool": None}


class Runtime:
    def __init__(self, model_dir: Path, device: str, allow_research_candidate=False):
        self.model_dir = model_dir
        self.device = device
        self.allow_research_candidate = allow_research_candidate
        self.model = None
        self.tokenizer = None
        self.status = "research"
        self.detail = "No released MiniMind model is installed"
        self._load()

    def _load(self):
        release_file = self.model_dir / "release.json"
        candidate_file = self.model_dir / "candidate.json"
        weights = self.model_dir / "model.safetensors"
        if not weights.exists():
            return
        metadata = None
        research = False
        if release_file.exists():
            metadata = json.loads(release_file.read_text(encoding="utf-8"))
            if metadata.get("status") != "released":
                self.detail = "Model exists but did not pass the release gates"
                return
        elif self.allow_research_candidate and candidate_file.exists():
            metadata = json.loads(candidate_file.read_text(encoding="utf-8"))
            if metadata.get("status") != "research-only" or metadata.get("release_eligible") is not False:
                self.detail = "Research candidate metadata is invalid"
                return
            research = True
        else:
            return
        startup_stage("verify_weights", bytes=weights.stat().st_size)
        if metadata.get("model_sha256") != sha256_file(weights):
            self.detail = "Model failed its integrity check"
            startup_stage("integrity_failed")
            return
        startup_stage("import_torch")
        import torch
        startup_stage("import_safetensors")
        from safetensors.torch import load_file
        startup_stage("import_transformers")
        from transformers import AutoTokenizer
        startup_stage("import_model")
        from model.model_minimind import MiniMindConfig, MiniMindForCausalLM

        startup_stage("construct_model", device=self.device)
        config = MiniMindConfig(**json.loads((self.model_dir / "config.json").read_text(encoding="utf-8")))
        dtype = torch.bfloat16 if self.device.startswith("cuda") else torch.float32
        model = MiniMindForCausalLM(config).to(device=self.device, dtype=dtype)
        model.load_state_dict(load_file(str(weights)), strict=True)
        model.eval()
        startup_stage("load_tokenizer")
        self.tokenizer = AutoTokenizer.from_pretrained(self.model_dir / "tokenizer", local_files_only=True)
        self.model = model
        self.status = "ready"
        self.detail = f"{'Research-only MiniMind' if research else 'MiniMind'} loaded on {self.device}"
        startup_stage("ready", device=self.device)

    def generate(self, payload):
        if self.model is None:
            return {"type": "error", "content": self.detail, "tool": None}
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            raise ValueError("messages must be a non-empty list")
        tools = payload.get("tools")
        if tools is not None and (not isinstance(tools, list) or any(not isinstance(tool, dict) for tool in tools)):
            raise ValueError("tools must be a list of definitions")
        routed_messages = list(messages)
        workspace = payload.get("workspace") or {}
        workspace_files = workspace.get("files") or []
        paths = [str(item.get("path", "")) for item in workspace_files if isinstance(item, dict) and item.get("path")]
        if paths:
            context = {"role": "system", "content": "项目文件索引（工具路径必须从这里选择；不要猜测路径）：\n" + "\n".join(paths[:240])}
            routed_messages = [context, *routed_messages]
        prompt = self.tokenizer.apply_chat_template(routed_messages, tokenize=False, add_generation_prompt=True, tools=tools or None)
        inputs = self.tokenizer(prompt, return_tensors="pt").to(self.device)
        output = self.model.generate(**inputs, max_new_tokens=512, do_sample=False, pad_token_id=self.tokenizer.eos_token_id)
        text = self.tokenizer.decode(output[0, inputs.input_ids.shape[1]:], skip_special_tokens=True)
        return parse_response(text)


class Handler(BaseHTTPRequestHandler):
    runtime = None

    def _json(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.send_header("cache-control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            return self._json(200, {"status": self.runtime.status, "detail": self.runtime.detail})
        self._json(404, {"error": {"message": "Not found"}})

    def do_POST(self):
        if self.path != "/generate":
            return self._json(404, {"error": {"message": "Not found"}})
        try:
            length = int(self.headers.get("content-length", "0"))
            if length > 1_048_576:
                return self._json(413, {"error": {"message": "Body too large"}})
            payload = json.loads(self.rfile.read(length))
            self._json(200, self.runtime.generate(payload))
        except (ValueError, json.JSONDecodeError) as error:
            self._json(400, {"error": {"message": str(error)}})
        except Exception as error:
            self._json(500, {"error": {"message": type(error).__name__}})

    def log_message(self, format, *args):
        print(json.dumps({"event": "model.http", "message": format % args}), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1", choices=["127.0.0.1", "localhost"])
    parser.add_argument("--port", type=int, default=3083)
    parser.add_argument("--model-dir", type=Path, default=ROOT / "model")
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--allow-research-candidate", action="store_true")
    args = parser.parse_args()
    Handler.runtime = Runtime(args.model_dir, args.device, args.allow_research_candidate)
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(json.dumps({"event": "model.ready", "host": args.host, "port": args.port, "status": Handler.runtime.status}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.server_close()


if __name__ == "__main__":
    main()
