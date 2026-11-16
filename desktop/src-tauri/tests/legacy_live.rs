use std::fs;
use std::path::PathBuf;
use ucode_portable_lib::legacy::LegacyAgentClient;

#[test]
fn live_legacy_agent_completes_an_approved_change() {
    if std::env::var("UCODE_LEGACY_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set UCODE_LEGACY_LIVE=1 for the real local-agent acceptance path");
        return;
    }

    let workspace = PathBuf::from(
        std::env::var("UCODE_LEGACY_WORKSPACE").expect("UCODE_LEGACY_WORKSPACE is required"),
    );
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        workspace.join("calculator.py"),
        "def add(a, b):\n    return a - b\n",
    )
    .unwrap();
    fs::write(
        workspace.join("test_calculator.py"),
        "from calculator import add\n\ndef test_add():\n    assert add(2, 3) == 5\n",
    )
    .unwrap();

    let client = LegacyAgentClient::new("http://127.0.0.1:3082").unwrap();
    assert!(
        client.health(),
        "PocketMind compatibility service is not ready"
    );
    client.select_project(workspace.to_str().unwrap()).unwrap();
    let ticket = client.create_ticket("修复 calculator.py 中的 add(a, b)，使它返回 a+b；写入后运行 pytest，测试通过后返回 final。").unwrap();
    let waiting = client.plan(&ticket).unwrap();
    let effect = waiting.effect.expect("local agent did not propose a write");
    assert_eq!(effect.path, "calculator.py");
    client.approve(&ticket, &effect.id, true, None).unwrap();
    let final_result = client.plan(&ticket).unwrap();
    assert_eq!(final_result.kind, "final");
    assert!(fs::read_to_string(workspace.join("calculator.py"))
        .unwrap()
        .contains("return a + b"));
}
