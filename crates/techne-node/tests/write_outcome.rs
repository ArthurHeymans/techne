//! Definitive node errors are different from a lost reply to a write.

use techne_node::{Node, RequestError, protocol::Expect};

#[test]
fn a_remote_write_error_is_definitive_and_a_later_write_can_succeed() {
    let node = Node::connect(&[env!("CARGO_BIN_EXE_techne-node").into()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing/file");
    let write = || node.write_file_tracked(path.as_os_str().as_encoded_bytes().to_vec(), b"text".to_vec(), Expect::Any);
    let result = techne_process::runtime().block_on(write());
    assert!(matches!(result, Err(RequestError::Failed(_))), "{result:?}");
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    assert!(techne_process::runtime().block_on(write()).unwrap().is_ok());
    assert_eq!(std::fs::read(path).unwrap(), b"text");
}
