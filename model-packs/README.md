# UCode Model Packs

Model packs are installed beside the portable application and are never committed to the source repository.

Each pack uses a stable lowercase directory ID and a `manifest.json`. UCode verifies the directory ID, relative path, expected byte length, and SHA-256 digest before listing the pack as usable. The current runtime accepts GGUF models through llama.cpp.

```text
model-packs/
  qwen2-5-coder-1-5b-q4-k-m/
    manifest.json
    model.gguf
    LICENSE.txt
    SOURCE.txt
```

Copy `example-manifest.json` into a pack directory and replace every placeholder using facts from the exact artifact. Do not set `releaseEligible` to true until license, source, checksum, memory requirements, and smoke tests have been reviewed.

The 1.5B profile is the portable default target. 3B/7B packs use the same mechanism but require more memory and are optional. UCode does not download a model without an explicit product workflow and does not imply that a configured endpoint is local.

