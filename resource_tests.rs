//! 外置资源完整性、归档边界、下载摘要和取消生命周期的离线验证。
use crate::{resource_archive, resource_download, resource_jobs::Jobs, resources};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn files(prefix: &str) -> Vec<(String, Vec<u8>)> {
    let html = r#"<html><head><script src="config.js"></script>
        <script src="res/class/Echo.js"></script><script src='res/class/EchoLive.js'></script>
        <script src="res/class/EchoLiveBroadcast.js"></script><script src=lib/jquery.js></script>
        <link href="res/style.css" rel="stylesheet"></head><body>
        <script src="res/script/live.js"></script></body></html>"#;
    [
        ("config.js", "const config = {data_version:1};"),
        ("live.html", html),
        ("res/class/Echo.js", "class Echo {}"),
        ("res/class/EchoLive.js", "class EchoLive {}"),
        (
            "res/class/EchoLiveBroadcast.js",
            "class EchoLiveBroadcast {}",
        ),
        ("res/script/live.js", "const echolive = new EchoLive();"),
        ("lib/jquery.js", "/* jquery */"),
        ("res/style.css", "body {}"),
    ]
    .into_iter()
    .map(|(name, text)| (format!("{prefix}{name}"), text.as_bytes().to_vec()))
    .collect()
}

fn write_directory(root: &Path) {
    for (name, bytes) in files("") {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn zip_bytes(entries: Vec<(String, Vec<u8>)>) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn validates_v1_without_language_and_references_folder_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("中文资源");
    write_directory(&folder);
    let value = resources::prepare(
        &temp.path().join("Assets"),
        "folder",
        Some(&folder),
        &AtomicBool::new(false),
        |_, _, _| {},
    )
    .unwrap();
    assert_eq!(value["data_version"], 1);
    assert_eq!(value["path"], json!(fs::canonicalize(&folder).unwrap()));
    assert_eq!(value["capabilities"]["live"], true);
    assert_eq!(value["capabilities"]["bridge"], false);
    assert!(!temp.path().join("Assets").exists());
}

#[test]
fn rejects_missing_runtime_files_empty_pages_and_missing_entry_references() {
    let temp = tempfile::tempdir().unwrap();
    write_directory(temp.path());
    fs::remove_file(temp.path().join("res/class/Echo.js")).unwrap();
    assert!(
        resources::validate(temp.path())
            .unwrap_err()
            .contains("Echo.js")
    );
    write_directory(temp.path());
    fs::write(temp.path().join("live.html"), "<html></html>").unwrap();
    assert!(
        resources::validate(temp.path())
            .unwrap_err()
            .contains("运行入口引用")
    );
    fs::write(temp.path().join("live.html"), "").unwrap();
    assert!(resources::validate(temp.path()).is_err());
}

#[test]
fn unknown_or_missing_config_versions_are_read_only() {
    let temp = tempfile::tempdir().unwrap();
    write_directory(temp.path());
    for script in ["const config = {};", "const config = {data_version:99};"] {
        fs::write(temp.path().join("config.js"), script).unwrap();
        let descriptor = resources::validate(temp.path()).unwrap();
        assert_eq!(descriptor["read_only"], true);
        assert_ne!(descriptor["compatibility_status"], "supported");
    }
}

#[test]
fn rejects_invalid_config_metadata_without_executing_javascript() {
    let temp = tempfile::tempdir().unwrap();
    write_directory(temp.path());
    fs::write(
        temp.path().join("config.js"),
        "const config = {data_version:17}; throw new Error('never execute');",
    )
    .unwrap();
    fs::create_dir_all(temp.path().join("res/data")).unwrap();
    fs::create_dir_all(temp.path().join("lang")).unwrap();
    fs::write(
        temp.path().join("lang/zho-Hans.js"),
        "const lang_zho_Hans = {config:{}};",
    )
    .unwrap();
    fs::write(
        temp.path().join("res/data/config_define.js"),
        "const db_config_define = [broken];",
    )
    .unwrap();
    assert!(
        resources::validate(temp.path())
            .unwrap_err()
            .contains("配置元数据校验失败")
    );
}

#[test]
fn imports_both_zip_layouts_and_reuses_content_digest_directory() {
    for prefix in ["", "echo-live-1.2.7/"] {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("中文发行包.zip");
        fs::write(&archive, zip_bytes(files(prefix))).unwrap();
        let assets = temp.path().join("custom-data/Assets");
        let prepare = || {
            resources::prepare(
                &assets,
                "import",
                Some(&archive),
                &AtomicBool::new(false),
                |_, _, _| {},
            )
            .unwrap()
        };
        let first = prepare();
        assert_eq!(first, prepare());
        assert_eq!(first["sha256"], first["id"]);
        assert!(
            Path::new(first["path"].as_str().unwrap())
                .join("live.html")
                .exists()
        );
        assert_eq!(fs::read_dir(assets.join("Echo-Live")).unwrap().count(), 1);
    }
}

#[test]
fn rejects_ambiguous_roots_and_cleans_failed_staging() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("ambiguous.zip");
    let mut entries = files("first/");
    entries.extend(files("second/"));
    fs::write(&archive, zip_bytes(entries)).unwrap();
    let assets = temp.path().join("Assets");
    let error = resources::prepare(
        &assets,
        "import",
        Some(&archive),
        &AtomicBool::new(false),
        |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.contains("多个"));
    assert_eq!(fs::read_dir(assets.join("Echo-Live")).unwrap().count(), 0);
}

#[test]
fn rejects_zip_traversal_windows_device_names_case_collisions_and_corruption() {
    let temp = tempfile::tempdir().unwrap();
    for names in [
        vec!["../escape.js"],
        vec!["C:/escape.js"],
        vec!["res/CON.js"],
        vec!["res/x.js", "RES/X.JS"],
    ] {
        let archive = temp.path().join("unsafe.zip");
        fs::write(
            &archive,
            zip_bytes(
                names
                    .into_iter()
                    .map(|name| (name.into(), b"data".to_vec()))
                    .collect(),
            ),
        )
        .unwrap();
        let destination = temp.path().join("output");
        assert!(
            resource_archive::extract(
                &archive,
                &destination,
                &AtomicBool::new(false),
                &|_, _, _| {}
            )
            .is_err()
        );
        if destination.exists() {
            fs::remove_dir_all(destination).unwrap();
        }
        assert!(!temp.path().join("escape.js").exists());
    }
    let archive = temp.path().join("unsafe.zip");
    fs::write(&archive, "damaged zip").unwrap();
    assert!(
        resource_archive::extract(
            &archive,
            &temp.path().join("output"),
            &AtomicBool::new(false),
            &|_, _, _| {}
        )
        .unwrap_err()
        .contains("ZIP 格式损坏")
    );
}

#[test]
fn cancelled_import_keeps_existing_assets_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let existing = temp.path().join("Assets/Echo-Live/existing");
    write_directory(&existing);
    let error = resources::prepare(
        &temp.path().join("Assets"),
        "import",
        Some(Path::new("absent.zip")),
        &AtomicBool::new(true),
        |_, _, _| {},
    )
    .unwrap_err();
    assert!(error.contains("取消"));
    assert!(existing.join("config.js").is_file());
}

#[test]
fn rejects_archive_symlinks_and_oversized_declared_entries() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("link.zip");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink(
            "res/link.js",
            "../outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    fs::write(&archive, writer.finish().unwrap().into_inner()).unwrap();
    let error = resource_archive::extract(
        &archive,
        &temp.path().join("links"),
        &AtomicBool::new(false),
        &|_, _, _| {},
    )
    .unwrap_err();
    assert!(error.contains("链接"));
    let mut bytes = zip_bytes(vec![("oversize.js".into(), b"x".to_vec())]);
    let central = bytes
        .windows(4)
        .position(|header| header == b"PK\x01\x02")
        .unwrap();
    bytes[central + 24..central + 28]
        .copy_from_slice(&((resources::MAX_FILE + 1) as u32).to_le_bytes());
    fs::write(&archive, bytes).unwrap();
    let error = resource_archive::extract(
        &archive,
        &temp.path().join("sizes"),
        &AtomicBool::new(false),
        &|_, _, _| {},
    )
    .unwrap_err();
    assert!(error.contains("128 MiB"));
}

fn reply(stream: &mut TcpStream, status: u16, body: &[u8], declared: usize) {
    let mut request = [0u8; 4096];
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let _ = stream.read(&mut request);
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

fn mock_release(
    digest: Option<String>,
    api_status: u16,
    truncated: bool,
) -> (String, thread::JoinHandle<()>, Vec<u8>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let body = zip_bytes(files(""));
    let mut release = json!({"draft":false,"prerelease":false,"tag_name":"1.2.7", "assets":[{
        "name":"echo-live-1.2.7.zip", "browser_download_url":format!("{address}/archive.zip"), "size":body.len()}]});
    if let Some(digest) = &digest {
        release["assets"][0]["digest"] = json!(format!("sha256:{digest}"));
    }
    let metadata = serde_json::to_vec(&release).unwrap();
    let sent = body.clone();
    let thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        reply(&mut stream, api_status, &metadata, metadata.len());
        if digest.is_some() && api_status == 200 {
            let (mut stream, _) = listener.accept().unwrap();
            reply(&mut stream, 200, &sent, sent.len() + usize::from(truncated));
        }
    });
    (format!("{address}/latest"), thread, body)
}

#[test]
fn download_requires_trusted_sha256_and_handles_api_errors_and_truncation() {
    let expected = resources::hex_digest(&Sha256::digest(zip_bytes(files(""))));
    for (digest, status, truncated, expected_error) in [
        (Some(expected.clone()), 200, false, None),
        (Some("0".repeat(64)), 200, false, Some("SHA256 校验失败")),
        (None, 200, false, Some("缺少可信的 SHA256")),
        (None, 403, false, Some("请求官方最新发行信息")),
        (Some(expected.clone()), 200, true, Some("读取发行 ZIP")),
    ] {
        let (api, server, _) = mock_release(digest, status, truncated);
        let temp = tempfile::tempdir().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let result = resource_download::download_from(
            &client,
            &api,
            &temp.path().join("download.zip"),
            &AtomicBool::new(false),
            &|_, _, _| {},
            false,
        );
        match expected_error {
            Some(message) => assert!(result.unwrap_err().contains(message)),
            None => assert_eq!(result.unwrap().0, expected),
        }
        server.join().unwrap();
    }
}

#[test]
fn cancellation_interrupts_stalled_network_request_promptly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/latest", listener.local_addr().unwrap());
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let server = thread::spawn(move || {
        let (_connection, _) = listener.accept().unwrap();
        signal.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(200));
    });
    let temp = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let result = resource_download::download_from(
        &reqwest::Client::new(),
        &api,
        &temp.path().join("download.zip"),
        &cancelled,
        &|_, _, _| {},
        false,
    );
    assert!(result.unwrap_err().contains("取消"));
    assert!(start.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
}

#[test]
fn job_start_returns_promptly_and_ready_cancel_discards_candidate() {
    let temp = tempfile::tempdir().unwrap();
    write_directory(temp.path());
    let mut jobs = Jobs::default();
    let start = Instant::now();
    let snapshot = jobs
        .start(
            temp.path().to_owned(),
            "folder".into(),
            Some(temp.path().to_owned()),
        )
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    let id = snapshot["id"].as_str().unwrap();
    loop {
        let state = jobs.status(id).unwrap();
        if state["busy"] == false {
            assert_eq!(state["stage"], "ready");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::sleep(Duration::from_millis(10));
    }
    let cancelled = jobs.cancel(Some(id)).unwrap();
    assert_eq!(cancelled["stage"], "cancelled");
    assert_eq!(cancelled["resource"], serde_json::Value::Null);
    assert!(jobs.status("stale-id").is_err());
    assert!(
        jobs.drain_logs()
            .iter()
            .any(|line| line.contains("准备完成"))
    );
    jobs.shutdown();
}

#[test]
fn modified_managed_resource_is_rejected_without_overwriting_existing_content() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("release.zip");
    fs::write(&archive, zip_bytes(files(""))).unwrap();
    let assets = temp.path().join("Assets");
    let prepare = || {
        resources::prepare(
            &assets,
            "import",
            Some(&archive),
            &AtomicBool::new(false),
            |_, _, _| {},
        )
    };
    let original = prepare().unwrap();
    let resource = Path::new(original["path"].as_str().unwrap());
    let custom = "const echolive = new EchoLive(); globalThis.externalModification = true;";
    fs::write(resource.join("res/script/live.js"), custom).unwrap();
    assert!(resources::validate(resource).is_ok());
    assert!(prepare().unwrap_err().contains("内容已被外部修改"));
    assert_eq!(
        fs::read_to_string(resource.join("res/script/live.js")).unwrap(),
        custom
    );
    assert_eq!(fs::read_dir(assets.join("Echo-Live")).unwrap().count(), 1);
}

#[test]
#[ignore = "需要访问 GitHub 官方最新发行；人工验收时显式运行"]
fn official_latest_release_download_and_validation() {
    let temp = tempfile::tempdir().unwrap();
    let result = resources::prepare(
        temp.path(),
        "download",
        None,
        &AtomicBool::new(false),
        |stage, bytes, total| {
            if !matches!(stage, "downloading" | "extracting") {
                eprintln!("{stage}: {bytes}/{total:?}");
            }
        },
    )
    .unwrap();
    assert_eq!(result["source"], "download");
    assert!(
        result["sha256"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    );
    assert_eq!(result["capabilities"]["live"], true);
    eprintln!("官方发行校验成功：{result}");
}
