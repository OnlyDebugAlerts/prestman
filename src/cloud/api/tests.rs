use super::*;
use crate::storage::Storage;

fn auth_response(account: Uuid, refresh_token: &str) -> AuthResponse {
    AuthResponse {
        access_token: "test-access-token".into(),
        refresh_token: refresh_token.into(),
        expires_in: 3600,
        user: AuthUser { id: account },
    }
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
