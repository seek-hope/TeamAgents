//! One local ChatGPT registration. All readers, writers and token exchanges lock the same
//! directory inode (never a profile inode, which changes on atomic replacement).
mod login;

use crate::{sandbox::fd_path, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DIR: &str = ".teamagents-auth";
const PROFILE: &str = "profile.json";
const TEMP: &str = "profile.tmp";
const LIMIT: u64 = 128 * 1024;
const REFRESH_MARGIN: u64 = 60;
const REFRESH_LIFETIME: u64 = 30 * 24 * 3600;
const SIGN_IN: &str = "Sign in with `teamagents auth login`.";
const RELOGIN: &str = "ChatGPT authorization has expired or been revoked. Sign in again with `teamagents auth login`.";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const IDENTITY_ONLY: &str =
    "ChatGPT sign-in has identity access only. Run `teamagents auth login` and allow plan usage for inference.";
const INVALID: &str = "Invalid ChatGPT credential profile. Check the private profile in $HOME/.teamagents-auth.";

// Deliberately no Debug on types carrying credentials.
#[derive(Clone, Serialize, Deserialize)]
struct Grant {
    client_id: String,
    subject: String,
    scopes: Vec<String>,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

impl Grant {
    fn allows_inference(&self) -> bool {
        self.scopes.iter().any(|s| s == DIRECT_SCOPE)
    }
}

#[derive(Serialize, Deserialize)]
struct Registration {
    client_id: String,
    subject: String,
}

#[derive(Serialize, Deserialize)]
struct Session {
    grant: Grant,
    refresh_expires_at: u64,
}

#[derive(Serialize, Deserialize)]
struct Profile {
    host_id: String,
    registration: Option<Registration>,
    session: Option<Session>,
}

fn store_error(e: io::Error) -> String {
    format!("ChatGPT credential storage: {e}")
}

fn now() -> Result<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).map_err(|_| "Check the system clock".into())
}

fn home() -> Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os("HOME").map(PathBuf::from) else { return Ok(None) };
    if !path.is_absolute() || path.parent().is_none() || path.components().any(|c| c == Component::ParentDir) {
        return Err("Set HOME to an absolute, trusted user directory for ChatGPT credential isolation".into());
    }
    Ok(Some(path))
}

/// Includes the prospective store before the first login. Call for both workspace and session.
/// The sandbox also reserves DIR by name, including when the daemon has a different HOME.
pub(crate) fn ensure_disjoint(path: &Path) -> Result<()> {
    disjoint(path, home()?.as_deref())
}

fn prospective_path(path: &Path) -> Result<PathBuf> {
    // Resolve each existing symlink before handling any subsequent `..`, including for a
    // --session path whose last few components do not exist yet.
    let absolute = std::path::absolute(path).map_err(store_error)?;
    let mut resolved = PathBuf::from("/");
    for component in absolute.components() {
        match component {
            Component::Normal(name) => {
                resolved.push(name);
                match fs::symlink_metadata(&resolved) {
                    Ok(m) if m.is_symlink() => resolved = resolved.canonicalize().map_err(store_error)?,
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(store_error(e)),
                    _ => {}
                }
            }
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {}
        }
    }
    Ok(resolved)
}

fn disjoint(path: &Path, home: Option<&Path>) -> Result<()> {
    const OVERLAP: &str = "Choose a workspace and session separate from the ChatGPT credential directory";
    let reserved = |p: &Path| p.components().any(|c| c.as_os_str().eq_ignore_ascii_case(DIR));
    let candidate = prospective_path(path)?;
    if reserved(path) || reserved(&candidate) {
        return Err(OVERLAP.into());
    }
    if let Some(home) = home {
        let store = prospective_path(home)?.join(DIR);
        if store.starts_with(&candidate) || candidate.starts_with(&store) {
            return Err(OVERLAP.into());
        }
        // A bind-mounted directory can have a second, unrelated canonical pathname.
        let inode = |p: &Path| -> Result<Option<(u64, u64)>> {
            match fs::symlink_metadata(p) {
                Ok(m) => Ok(Some((m.dev(), m.ino()))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(store_error(e)),
            }
        };
        for (root, child) in [(&store, &candidate), (&candidate, &store)] {
            if let Some(id) = inode(root)? {
                for ancestor in child.ancestors() {
                    if inode(ancestor)? == Some(id) {
                        return Err(OVERLAP.into());
                    }
                }
            }
        }
    }
    Ok(())
}

fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)
}

fn trusted_home(path: &Path) -> Result<File> {
    // Pin each directory before descending. Only root and the user may control the path.
    // A root-owned sticky ancestor such as /tmp may contain a privately owned test HOME.
    let uid = unsafe { libc::geteuid() };
    let mut dir = open_dir(Path::new("/")).map_err(store_error)?;
    for c in path.components() {
        match c {
            Component::RootDir => continue,
            Component::Normal(name) => dir = open_dir(&fd_path(&dir).join(name)).map_err(store_error)?,
            _ => return Err("Invalid ChatGPT credential home".into()),
        }
        let m = dir.metadata().map_err(store_error)?;
        if (m.uid() != 0 && m.uid() != uid) || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0)) {
            return Err("Unsafe ownership or write permissions in the ChatGPT credential home path".into());
        }
    }
    let m = dir.metadata().map_err(store_error)?;
    if m.uid() != uid || m.mode() & 0o022 != 0 {
        return Err("HOME must be owned by you and writable only by you".into());
    }
    Ok(dir)
}

struct Locked(File);

impl Locked {
    fn open(home: &Path, create: bool) -> Result<Option<Self>> {
        let parent = trusted_home(home)?;
        let path = fd_path(&parent).join(DIR);
        if create {
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => parent.sync_all().map_err(store_error)?,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        let dir = match open_dir(&path) {
            Err(e) if !create && e.kind() == io::ErrorKind::NotFound => return Ok(None),
            other => other.map_err(store_error)?,
        };
        let m = dir.metadata().map_err(store_error)?;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
            return Err("The ChatGPT credential directory must be owned by you with mode 0700".into());
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match dir.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(
                        "ChatGPT credentials are busy. Retry after the current sign-in, refresh or logout completes."
                            .into(),
                    )
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(store_error(e)),
            }
        }
        // A crash before rename may leave a private temp file. No live writer can own it now.
        match fs::remove_file(fd_path(&dir).join(TEMP)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(store_error(e)),
            _ => {}
        }
        Ok(Some(Self(dir)))
    }

    fn load(&self) -> Result<Option<Profile>> {
        let path = fd_path(&self.0).join(PROFILE);
        // Pin, stat, then read the same regular file. Opening an unexpected device or FIFO
        // directly could have side effects or block before we could check its metadata.
        let pin = match OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_NOFOLLOW).open(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            other => other.map_err(store_error)?,
        };
        let m = pin.metadata().map_err(store_error)?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o600 || m.nlink() != 1 {
            return Err("The ChatGPT profile must be a private, owned, single-link regular file with mode 0600".into());
        }
        let mut data = Vec::new();
        File::open(fd_path(&pin)).map_err(store_error)?.take(LIMIT + 1).read_to_end(&mut data).map_err(store_error)?;
        if data.len() as u64 > LIMIT {
            return Err(INVALID.into());
        }
        let profile: Profile = serde_json::from_slice(&data).map_err(|_| INVALID)?;
        if uuid::Uuid::parse_str(&profile.host_id).is_err() {
            return Err(INVALID.into());
        }
        if let Some(session) = &profile.session {
            profile.check_grant(&session.grant)?;
            if profile.registration.is_none() || session.refresh_expires_at == 0 {
                return Err(INVALID.into());
            }
        }
        Ok(Some(profile))
    }

    fn save(&self, profile: &Profile) -> Result<()> {
        let bytes = serde_json::to_vec(profile).map_err(|_| INVALID)?;
        if bytes.len() as u64 > LIMIT {
            return Err(INVALID.into());
        }
        let dir = fd_path(&self.0);
        let pending = dir.join(TEMP);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&pending)
            .map_err(store_error)?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&pending, dir.join(PROFILE)))
            .and_then(|_| self.0.sync_all());
        if result.is_err() {
            let _ = fs::remove_file(pending);
        }
        result.map_err(store_error)
    }

    fn sign_in(&self, authorize: impl FnOnce(&str, Option<&Registration>) -> Result<Grant>) -> Result<bool> {
        let mut profile = match self.load()? {
            Some(profile) => profile,
            None => {
                let profile = Profile { host_id: uuid::Uuid::new_v4().to_string(), registration: None, session: None };
                self.save(&profile)?; // Keep the host identity even across interrupted browser sign-ins.
                profile
            }
        };
        let grant = authorize(&profile.host_id, profile.registration.as_ref())?;
        let can_infer = grant.allows_inference();
        profile.install(grant, now()?)?;
        self.save(&profile)?;
        Ok(can_infer)
    }

    fn token(&self, at: u64, refresh: impl FnOnce(&Grant) -> Result<Grant>) -> Result<String> {
        let mut profile = self.load()?.ok_or(SIGN_IN)?;
        let session = profile.session.as_ref().ok_or(SIGN_IN)?;
        if !session.grant.allows_inference() {
            return Err(IDENTITY_ONLY.into());
        }
        if session.grant.expires_at > at.saturating_add(REFRESH_MARGIN) {
            return Ok(session.grant.access_token.clone());
        }
        let renewed = if session.refresh_expires_at <= at { Err(RELOGIN.into()) } else { refresh(&session.grant) };
        match renewed {
            Ok(grant) => {
                profile.install(grant, at)?;
                self.save(&profile)?; // Publish both rotated tokens before releasing the directory lock.
                let grant = profile.session.unwrap().grant;
                if !grant.allows_inference() {
                    return Err(IDENTITY_ONLY.into());
                }
                Ok(grant.access_token)
            }
            Err(e) if e == RELOGIN => {
                profile.session = None;
                self.save(&profile)?;
                Err(e)
            }
            Err(e) => Err(e), // Preserve credentials on transient provider or network errors.
        }
    }

    fn sign_out(&self, revoke: impl FnOnce(&Grant) -> bool) -> Result<()> {
        let Some(mut profile) = self.load()? else { return Ok(()) };
        let Some(session) = profile.session.take() else { return Ok(()) };
        let confirmed = revoke(&session.grant);
        self.save(&profile)?;
        if confirmed {
            Ok(())
        } else {
            Err("Signed out locally. Remote revocation is unconfirmed. Disconnect the app in ChatGPT Settings.".into())
        }
    }
}

impl Profile {
    fn check_grant(&self, grant: &Grant) -> Result<()> {
        if grant.client_id.is_empty()
            || grant.subject.is_empty()
            || grant.expires_at == 0
            || [&grant.access_token, &grant.refresh_token]
                .iter()
                .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err(INVALID.into());
        }
        if self.registration.as_ref().is_some_and(|r| r.client_id != grant.client_id || r.subject != grant.subject) {
            return Err("The returned authorization does not match the saved ChatGPT registration".into());
        }
        Ok(())
    }

    fn install(&mut self, grant: Grant, at: u64) -> Result<()> {
        self.check_grant(&grant)?;
        if grant.expires_at <= at {
            return Err("ChatGPT returned an expired access token. Check the system clock".into());
        }
        self.registration = Some(Registration { client_id: grant.client_id.clone(), subject: grant.subject.clone() });
        self.session = Some(Session { grant, refresh_expires_at: at.saturating_add(REFRESH_LIFETIME) });
        Ok(())
    }
}

fn existing() -> Result<Option<Locked>> {
    match home()? {
        Some(path) => Locked::open(&path, false),
        None => Ok(None),
    }
}

pub(crate) fn login() -> Result<()> {
    let home = home()?.ok_or("Set HOME to a trusted user directory before signing in")?;
    let can_infer =
        Locked::open(&home, true)?.ok_or("Cannot open ChatGPT credential directory")?.sign_in(login::authorize)?;
    if can_infer {
        println!("Signed in to ChatGPT.");
    } else {
        println!("{IDENTITY_ONLY}");
    }
    Ok(())
}

pub(crate) fn logout() -> Result<()> {
    if let Some(store) = existing()? {
        store.sign_out(revoke)?;
    }
    println!("Signed out of ChatGPT.");
    Ok(())
}

pub(crate) fn status() -> Result<()> {
    let session = existing()?.map(|s| s.load()).transpose()?.flatten().and_then(|p| p.session);
    match session {
        None => println!("Signed out of ChatGPT. {SIGN_IN}"),
        Some(s) if s.refresh_expires_at <= now()? => println!("{RELOGIN}"),
        Some(s) if !s.grant.allows_inference() => println!("{IDENTITY_ONLY}"),
        Some(_) => println!("Signed in to ChatGPT."),
    }
    Ok(())
}

pub(crate) fn configured() -> Result<bool> {
    Ok(existing()?.map(|s| s.load()).transpose()?.flatten().is_some_and(|p| p.session.is_some()))
}

pub(crate) fn access_token() -> Result<String> {
    existing()?.ok_or(SIGN_IN)?.token(now()?, login::refresh)
}

const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const REVOCATION: &str = "https://auth.openai.com/api/accounts/oauth/revoke";

// RFC 7009 via production OIDC metadata. Never send a token to a redirect or an arbitrary
// advertised URL. Surface only local guidance on failure, never provider bodies or requests.
fn revoke(grant: &Grant) -> bool {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .max_idle_connections(0)
        .timeout(Duration::from_secs(8))
        .build();
    let attempt = || -> std::result::Result<(), bool> {
        fn success(
            result: std::result::Result<ureq::Response, ureq::Error>,
        ) -> std::result::Result<ureq::Response, bool> {
            match result {
                Ok(response) if response.status() == 200 => Ok(response),
                Err(ureq::Error::Transport(_)) | Err(ureq::Error::Status(500..=599, _)) => Err(true),
                _ => Err(false),
            }
        }
        let mut data = Vec::new();
        success(agent.get(DISCOVERY).call())?.into_reader().take(LIMIT + 1).read_to_end(&mut data).map_err(|_| true)?;
        if data.len() as u64 > LIMIT {
            return Err(false);
        }
        let config: serde_json::Value = serde_json::from_slice(&data).map_err(|_| false)?;
        if config["revocation_endpoint"].as_str() != Some(REVOCATION) {
            return Err(false);
        }
        success(agent.post(REVOCATION).send_form(&[
            ("token", &grant.refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", &grant.client_id),
        ]))?;
        Ok(())
    };
    for delay_ms in [0, 250, 1000] {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        match attempt() {
            Ok(()) => return true,
            Err(false) => break,
            Err(true) => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::Command;

    struct Home(PathBuf);

    impl Home {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("teamagents-oauth-test-{}", uuid::Uuid::new_v4()));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }

        fn lock(&self) -> Locked {
            Locked::open(&self.0, true).unwrap().unwrap()
        }

        fn profile(&self) -> PathBuf {
            self.0.join(DIR).join(PROFILE)
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn fake(name: &str, expires_at: u64) -> Grant {
        Grant {
            subject: "fixture-user".into(),
            client_id: "fixture-client".into(),
            access_token: format!("fake-access-{name}"),
            refresh_token: format!("fake-refresh-{name}"),
            scopes: vec![DIRECT_SCOPE.into()],
            expires_at,
        }
    }

    // Fixed times and all HTTP-facing calls replaced with closures.
    fn seed(store: &Locked) {
        let mut p = Profile { host_id: uuid::Uuid::new_v4().to_string(), registration: None, session: None };
        p.install(fake("old", 2000), 1000).unwrap();
        store.save(&p).unwrap();
    }

    #[test]
    fn rejects_overlap_even_before_login_and_through_aliases() {
        let home = Home::new();
        let check = |path: &Path| disjoint(path, Some(&home.0));
        assert!(check(&home.0).is_err());
        assert!(check(home.0.parent().unwrap()).is_err());
        assert!(check(&home.profile()).is_err());
        assert!(check(&home.0.join("project")).is_ok());
        assert!(Locked::open(&home.0, false).unwrap().is_none());
        let alias = Home::new();
        symlink(&home.0, alias.0.join("link")).unwrap();
        assert!(check(&alias.0.join("link")).is_err());
        assert!(check(&alias.0.join("link/no-dir/../.teamagents-auth/session")).is_err());
        assert!(!home.0.join(DIR).exists());
    }

    #[test]
    fn permissions_and_links_fail_closed() {
        let home = Home::new();
        let other = Home::new();
        let dir = home.0.join(DIR);
        symlink(&other.0, &dir).unwrap();
        assert!(Locked::open(&home.0, true).is_err());
        fs::remove_file(&dir).unwrap();
        let store = home.lock();
        seed(&store);
        let path = home.profile();
        assert_eq!(dir.metadata().unwrap().mode() & 0o7777, 0o700);
        assert_eq!(path.metadata().unwrap().mode() & 0o7777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.load().is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let linked = other.0.join("linked");
        fs::hard_link(&path, &linked).unwrap();
        assert!(store.load().is_err());
        fs::remove_file(&path).unwrap();
        symlink(&linked, &path).unwrap();
        assert!(store.load().is_err());
        drop(store);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(Locked::open(&home.0, false).is_err());
        symlink(&home.0, other.0.join("alias")).unwrap();
        assert!(Locked::open(&other.0.join("alias"), false).is_err());
    }

    #[test]
    fn logout_clears_tokens_and_keeps_host_and_registration_for_returning_user() {
        for confirmed in [true, false] {
            let home = Home::new();
            let store = home.lock();
            let mut host = String::new();
            assert!(store
                .sign_in(|id, previous| {
                    host = id.to_owned();
                    assert!(previous.is_none());
                    Err("cancelled".into())
                })
                .is_err());
            assert!(store
                .sign_in(|id, previous| {
                    assert_eq!(id, host);
                    assert!(previous.is_none());
                    Ok(fake("initial", now()? + 3600))
                })
                .unwrap());
            let result = store.sign_out(|grant| {
                assert_eq!(grant.refresh_token, "fake-refresh-initial");
                confirmed
            });
            if confirmed {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("ChatGPT Settings"));
            }
            assert!(!fs::read_to_string(home.profile()).unwrap().contains("fake-"));
            assert!(store.sign_out(|_| panic!("already signed out")).is_ok());
            assert!(store.token(0, |_| panic!()).unwrap_err().contains("auth login"));
            let previous_account = |id: &str, previous: Option<&Registration>| {
                assert_eq!(id, host);
                let registration = previous.unwrap();
                assert_eq!(registration.client_id, "fixture-client");
                assert_eq!(registration.subject, "fixture-user");
            };
            assert!(store
                .sign_in(|id, previous| {
                    previous_account(id, previous);
                    let mut g = fake("other", now()? + 3600);
                    g.subject = "other-account".into();
                    Ok(g)
                })
                .is_err());
            assert!(store
                .sign_in(|id, previous| {
                    previous_account(id, previous);
                    Ok(fake("returning", now()? + 3600))
                })
                .unwrap());
        }
    }

    #[test]
    fn transient_refresh_retains_tokens_but_expiry_and_revocation_clear_them() {
        let home = Home::new();
        let store = home.lock();
        seed(&store);
        assert_eq!(store.token(1900, |_| panic!("fresh")).unwrap(), "fake-access-old");
        assert_eq!(store.token(1999, |_| Err("offline".into())).unwrap_err(), "offline");
        assert_eq!(
            store
                .token(1999, |g| {
                    assert_eq!(g.refresh_token, "fake-refresh-old");
                    Ok(fake("rotated", 3000))
                })
                .unwrap(),
            "fake-access-rotated"
        );
        assert_eq!(
            store
                .token(3000, |g| {
                    assert_eq!(g.refresh_token, "fake-refresh-rotated");
                    Err(RELOGIN.into())
                })
                .unwrap_err(),
            RELOGIN
        );
        assert!(store.load().unwrap().unwrap().session.is_none());
        seed(&store);
        assert_eq!(store.token(1000 + REFRESH_LIFETIME, |_| panic!("expired")).unwrap_err(), RELOGIN);
        let p = store.load().unwrap().unwrap();
        assert!(p.session.is_none() && p.registration.is_some());
    }

    #[test]
    fn scope_loss_persists_rotation_and_blocks_access_until_reauthorization() {
        let home = Home::new();
        let store = home.lock();
        seed(&store);
        assert_eq!(
            store
                .token(1999, |_| {
                    let mut reduced = fake("reduced", 4000);
                    reduced.scopes = vec!["openid".into()];
                    Ok(reduced)
                })
                .unwrap_err(),
            IDENTITY_ONLY
        );
        assert_eq!(store.token(3000, |_| panic!("no direct scope")).unwrap_err(), IDENTITY_ONLY);
        assert_eq!(store.load().unwrap().unwrap().session.unwrap().grant.refresh_token, "fake-refresh-reduced");
        assert!(!store
            .sign_in(|_, _| {
                let mut g = fake("identity", now()? + 3600);
                g.scopes.clear();
                Ok(g)
            })
            .unwrap());
    }

    #[test]
    fn directory_lock_has_a_bounded_wait() {
        let home = Home::new();
        let _guard = home.lock();
        let start = std::time::Instant::now();
        let error = Locked::open(&home.0, false).err().unwrap();
        assert!(error.contains("Retry"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn processes_share_one_rotated_refresh_token() {
        const CHILD: &str = "TEAMAGENTS_UNIT_TEST_REFRESH_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap());
            assert!(home.file_name().unwrap().to_string_lossy().starts_with("teamagents-oauth-test-"));
            let store = Locked::open(&home, false).unwrap().unwrap();
            assert_eq!(
                store
                    .token(1999, |g| {
                        assert_eq!(g.refresh_token, "fake-refresh-old");
                        File::create_new(home.join("once")).unwrap();
                        std::thread::sleep(Duration::from_millis(100));
                        Ok(fake("new", 4000))
                    })
                    .unwrap(),
                "fake-access-new"
            );
            return;
        }
        let home = Home::new();
        seed(&home.lock());
        let children: Vec<_> = (0..4)
            .map(|_| {
                Command::new(std::env::current_exe().unwrap())
                    .env_clear()
                    .env(CHILD, "1")
                    .env("HOME", &home.0)
                    .args(["--exact", "oauth::tests::processes_share_one_rotated_refresh_token"])
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert!(home.0.join("once").is_file());
        assert_eq!(home.lock().load().unwrap().unwrap().session.unwrap().grant.refresh_token, "fake-refresh-new");
    }
}
