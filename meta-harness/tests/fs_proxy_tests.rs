use std::fs;
use tempfile::tempdir;

use meta_harness::fs_proxy::{
    read_client_text_file, write_client_text_file, ReadTextFileRequest, WriteTextFileRequest,
};

#[test]
fn test_reads_and_writes_client_files_within_session_roots() {
    let cwd_dir = tempdir().unwrap();
    let outside_dir = tempdir().unwrap();
    let attachments_dir = tempdir().unwrap();

    let cwd = cwd_dir.path().to_path_buf();
    let outside = outside_dir.path().to_path_buf();
    let attachments = attachments_dir.path().to_path_buf();
    let allowed_roots = vec![cwd.clone(), attachments];

    let notes_file = cwd.join("notes.txt");
    fs::write(&notes_file, "one\ntwo\nthree\n").unwrap();

    // 1. Full read
    let full = read_client_text_file(
        &allowed_roots,
        &ReadTextFileRequest {
            path: notes_file.to_str().unwrap().to_string(),
            line: None,
            limit: None,
        },
    )
    .expect("full read should succeed");
    assert_eq!(full.content, "one\ntwo\nthree\n");

    // 2. Windowed read with line (1-indexed) and limit
    let window = read_client_text_file(
        &allowed_roots,
        &ReadTextFileRequest {
            path: notes_file.to_str().unwrap().to_string(),
            line: Some(2),
            limit: Some(1),
        },
    )
    .expect("windowed read should succeed");
    assert_eq!(window.content, "two");

    // 3. Write nested file within session root
    let nested_new = cwd.join("nested").join("new.txt");
    write_client_text_file(
        &allowed_roots,
        &WriteTextFileRequest {
            path: nested_new.to_str().unwrap().to_string(),
            content: "created".to_string(),
        },
    )
    .expect("nested write should succeed");
    assert_eq!(fs::read_to_string(&nested_new).unwrap(), "created");

    // 4. Reject write outside session roots
    let escape_file = outside.join("escape.txt");
    let escape_result = write_client_text_file(
        &allowed_roots,
        &WriteTextFileRequest {
            path: escape_file.to_str().unwrap().to_string(),
            content: "nope".to_string(),
        },
    );
    assert!(escape_result.is_err());
    assert!(!escape_file.exists());

    // 5. Missing file read returns error
    let missing_file = cwd.join("missing.txt");
    let missing_result = read_client_text_file(
        &allowed_roots,
        &ReadTextFileRequest {
            path: missing_file.to_str().unwrap().to_string(),
            line: None,
            limit: None,
        },
    );
    assert!(missing_result.is_err());
}

#[test]
fn test_enforces_max_file_size_and_symlink_escape() {
    let cwd_dir = tempdir().unwrap();
    let outside_dir = tempdir().unwrap();
    let cwd = cwd_dir.path().to_path_buf();
    let outside = outside_dir.path().to_path_buf();
    let allowed_roots = vec![cwd.clone()];

    // Test symlink escape
    let secret_file = outside.join("secret.env");
    fs::write(&secret_file, "SECRET=123").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let symlink_in_cwd = cwd.join("escape_symlink");
        let _ = symlink(&outside, &symlink_in_cwd);

        let target_via_symlink = symlink_in_cwd.join("secret.env");
        let read_res = read_client_text_file(
            &allowed_roots,
            &ReadTextFileRequest {
                path: target_via_symlink.to_str().unwrap().to_string(),
                line: None,
                limit: None,
            },
        );
        assert!(read_res.is_err(), "Symlink escape outside root must be rejected");
    }

    // Relative path resolving into root
    let rel_file = cwd.join("relative.txt");
    fs::write(&rel_file, "hello").unwrap();
    let rel_read = read_client_text_file(
        &allowed_roots,
        &ReadTextFileRequest {
            path: "relative.txt".to_string(),
            line: None,
            limit: None,
        },
    )
    .expect("relative path should resolve inside root");
    assert_eq!(rel_read.content, "hello");
}
