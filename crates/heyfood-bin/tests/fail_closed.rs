use std::process::Command;

#[test]
fn qualification_binary_refuses_before_touching_terminal_state() {
    let output = Command::new(env!("CARGO_BIN_EXE_heyfood"))
        .output()
        .expect("qualification binary should run");

    assert_eq!(output.status.code(), Some(78));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(stderr.contains("cannot start"));
    assert!(!stderr.contains('\u{1b}'), "must not enter terminal modes");
    assert!(!stderr.contains("██"), "must not emit a giant banner");
}
