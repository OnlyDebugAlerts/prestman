use super::*;
use crate::storage::Storage;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

fn auth_response(account: Uuid, refresh_token: &str) -> AuthResponse {
    AuthResponse {
        access_token: "test-access-token".into(),
        refresh_token: refresh_token.into(),
        expires_in: 3600,
        user: AuthUser { id: account },
    }
}

#[test]
fn cloud_requests_send_the_public_key_only_when_configured() -> Result<()> {
    for key in ["", "sb_publishable_test"] {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let origin = format!("http://{}", listener.local_addr()?);
        let config = CloudConfig::new(CloudServer::parse(&origin, key)?)?;
        let server = std::thread::spawn(move || -> Result<Vec<String>> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&mut stream);
            let mut headers = Vec::new();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                    break;
                }
                let line = line.trim_end().to_ascii_lowercase();
                if let Some(length) = line.strip_prefix("content-length:") {
                    content_length = length.trim().parse()?;
                }
                headers.push(line);
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body)?;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            Ok(headers)
        });
        let response = config.post(
            "/rest/v1/rpc/prestman_workspaces",
            Some("session-token"),
            &json!({}),
        )?;
        assert!(response.status().is_success());
        let headers = server.join().expect("test server did not panic")?;
        assert!(headers.contains(&"authorization: bearer session-token".into()));
        assert_eq!(
            headers
                .iter()
                .find_map(|header| header.strip_prefix("apikey: ")),
            (!key.is_empty()).then_some(key),
        );
    }
    Ok(())
}

#[test]
fn saves_the_latest_cloud_token_across_reopens_and_removes_it_on_sign_out() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (storage, _) = Storage::open_in(root.path().into())?;
    let origin = "https://cloud.example.test";
    let config = CloudConfig::new(CloudServer::parse(origin, "sb_publishable_test")?)?;
    let account = Uuid::new_v4();
    let store = storage.cloud_session_store();
    assert!(store.load(origin)?.is_none());

    let api = config.start_session(auth_response(account, "initial"), Some(store))?;
    let store = storage.cloud_session_store();
    assert_eq!(store.load(origin)?.as_deref(), Some("initial"));

    {
        let mut session = api.session.lock().unwrap();
        api.apply_refresh(session.as_mut().unwrap(), auth_response(account, "rotated"))?;
    }
    drop(api);
    drop(store);
    drop(storage);

    let (storage, _) = Storage::open_in(root.path().into())?;
    let store = storage.cloud_session_store();
    assert_eq!(store.load(origin)?.as_deref(), Some("rotated"));
    assert!(
        store
            .load("https://different-cloud.example.test")?
            .is_none()
    );

    let file = root.path().join("cloud-session.json");
    assert!(!std::fs::read_to_string(&file)?.contains("test-access-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file)?.permissions().mode() & 0o077, 0);
    }

    let api = config.start_session(auth_response(account, "restored"), Some(store))?;
    let background_api = api.clone();
    api.sign_out()?;
    assert!(!file.exists());
    assert!(storage.cloud_session_store().load(origin)?.is_none());
    assert!(background_api.workspaces().is_err());
    Ok(())
}
