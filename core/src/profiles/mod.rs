//! The AWS CLI's profiles: the ones in its [shared config and credentials files](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html),
//! found where the CLI finds them, listed, added, edited and removed without touching anything else in those files
//! (`ini`). Each profile points at one radosgw account through its keys and `endpoint_url`.

mod ini;

use ini::Document;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Why a profile couldn't be read, written or accepted.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// Neither `AWS_CONFIG_FILE`/`AWS_SHARED_CREDENTIALS_FILE` nor a home folder to find `~/.aws` in.
    #[error("no home folder to find ~/.aws in: set AWS_CONFIG_FILE and AWS_SHARED_CREDENTIALS_FILE")]
    NoHome,
    /// A file that exists couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// A name the files can't hold as a section, or one reserved for other sections.
    #[error("invalid profile name {0:?}: use letters, digits, '-', '_', '.' or '@'")]
    InvalidName(String),
    /// An endpoint that isn't an `http://` or `https://` URL.
    #[error("invalid endpoint {0:?}: it must start with http:// or https:// and have a host")]
    InvalidEndpoint(String),
    /// A region with characters a region can't have.
    #[error("invalid region {0:?}")]
    InvalidRegion(String),
    /// An access key or secret that's empty or has whitespace in it.
    #[error("invalid {0}: it can't be empty or contain spaces")]
    InvalidKey(&'static str),
    /// Saving under a name another profile already has.
    #[error("a profile named {0:?} already exists")]
    Exists(String),
    /// Editing or removing a profile that's gone.
    #[error("no profile named {0:?}")]
    NotFound(String),
}

/// Where the profile settings come from for one profile's credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credentials {
    /// An access key and secret in the credentials or config file: what rs3manager writes, and can edit.
    Keys,
    /// Something the SDK resolves itself (SSO, `credential_process`, a role, a web identity): used as is, not
    /// editable here.
    External,
    /// None set: connecting fails until keys are added.
    Missing,
}

/// A profile as the files describe it. The secret key is never read into it, so it can't leak into the UI or logs;
/// `ProfileEdit::secret_access_key` left out keeps the stored one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The profile's name, as `aws --profile` takes it.
    pub name: String,
    /// The S3 endpoint, the radosgw's URL.
    pub endpoint_url: Option<String>,
    /// The signing region; radosgw's default zonegroup is `default`.
    pub region: Option<String>,
    /// The access key id, when the profile has static keys.
    pub access_key_id: Option<String>,
    /// Where its credentials come from.
    pub credentials: Credentials,
}

/// A profile's settings as typed, checked once by [`ProfileEdit::new`] before anything reaches the files: values
/// can't carry a newline or bracket that would start another key or section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEdit {
    /// The profile's name.
    name: String,
    /// The endpoint URL.
    endpoint_url: String,
    /// The region, `default` when left empty.
    region: String,
    /// A new access key id, or `None` to keep the stored one.
    access_key_id: Option<String>,
    /// A new secret, or `None` to keep the stored one.
    secret_access_key: Option<String>,
}

/// The region a profile gets when none is typed: radosgw's default zonegroup, which `dev/rgw.sh` profiles use.
pub const DEFAULT_REGION: &str = "default";

impl ProfileEdit {
    /// Checks typed settings. Empty `access_key_id`/`secret_access_key` keep the stored ones; an empty region is
    /// [`DEFAULT_REGION`].
    pub fn new(
        name: &str,
        endpoint_url: &str,
        region: &str,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<Self, ProfileError> {
        let name = name.trim();
        if !valid_name(name) {
            return Err(ProfileError::InvalidName(name.to_owned()));
        }
        let endpoint_url = endpoint_url.trim().trim_end_matches('/');
        if !valid_endpoint(endpoint_url) {
            return Err(ProfileError::InvalidEndpoint(endpoint_url.to_owned()));
        }
        let region = match region.trim() {
            "" => DEFAULT_REGION,
            r if r.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => r,
            r => return Err(ProfileError::InvalidRegion(r.to_owned())),
        };
        Ok(Self {
            name: name.to_owned(),
            endpoint_url: endpoint_url.to_owned(),
            region: region.to_owned(),
            access_key_id: key(access_key_id, "access key id")?,
            secret_access_key: key(secret_access_key, "secret access key")?,
        })
    }

    /// The checked name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A typed key: `None` when empty (keep the stored one), an error with whitespace inside.
fn key(value: &str, what: &'static str) -> Result<Option<String>, ProfileError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(char::is_whitespace) {
        return Err(ProfileError::InvalidKey(what));
    }
    Ok(Some(value.to_owned()))
}

/// Whether `name` can be a profile's section in both files: the characters the AWS CLI's own `configure` accepts in
/// practice, and none that would end a header or line.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
}

/// Whether `url` is `http(s)://host[...]` with no whitespace.
fn valid_endpoint(url: &str) -> bool {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/') && !r.chars().any(|c| c.is_whitespace() || c == '#'))
}

/// The two files, where the AWS CLI looks for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    /// The config file: `AWS_CONFIG_FILE`, else `~/.aws/config`.
    pub config: PathBuf,
    /// The credentials file: `AWS_SHARED_CREDENTIALS_FILE`, else `~/.aws/credentials`.
    pub credentials: PathBuf,
}

impl Files {
    /// The files the AWS CLI uses in this environment.
    pub fn from_env() -> Result<Self, ProfileError> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = || std::env::home_dir().map(|h| h.join(".aws")).ok_or(ProfileError::NoHome);
        Ok(Self {
            config: match var("AWS_CONFIG_FILE") {
                Some(path) => path,
                None => home()?.join("config"),
            },
            credentials: match var("AWS_SHARED_CREDENTIALS_FILE") {
                Some(path) => path,
                None => home()?.join("credentials"),
            },
        })
    }

    /// Both files in `dir`, for tests and for a folder of its own.
    pub fn in_dir(dir: &Path) -> Self {
        Self { config: dir.join("config"), credentials: dir.join("credentials") }
    }

    /// Both documents, empty for a missing file.
    fn read(&self) -> Result<(Document, Document), ProfileError> {
        Ok((read(&self.config)?, read(&self.credentials)?))
    }

    /// Every profile in either file, in the order they first appear (config first).
    pub fn load(&self) -> Result<Vec<Profile>, ProfileError> {
        let (config, credentials) = self.read()?;
        let mut names: Vec<String> = config.sections().into_iter().filter_map(profile_of_config_header).collect();
        for name in credentials.sections() {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        Ok(names.into_iter().map(|name| profile(&config, &credentials, name)).collect())
    }

    /// Saves `edit` as a new profile (`original` `None`) or over the profile `original`, renaming it when the name
    /// changed. Only the keys rs3manager owns are written; everything else in both files stays.
    pub fn save(&self, original: Option<&str>, edit: &ProfileEdit) -> Result<(), ProfileError> {
        let (mut config, mut credentials) = self.read()?;
        let existing: Vec<String> = self.load()?.into_iter().map(|p| p.name).collect();
        let taken = |name: &str| existing.iter().any(|n| n == name);
        if original != Some(edit.name.as_str()) && taken(&edit.name) {
            return Err(ProfileError::Exists(edit.name.clone()));
        }
        if let Some(from) = original {
            if !taken(from) {
                return Err(ProfileError::NotFound(from.to_owned()));
            }
            if from != edit.name {
                config.rename_section(&config_header(from), &config_header(&edit.name));
                credentials.rename_section(from, &edit.name);
            }
        }
        let header = config_header(&edit.name);
        config.set(&header, "endpoint_url", Some(&edit.endpoint_url));
        config.set(&header, "region", Some(&edit.region));
        for (key, value) in
            [("aws_access_key_id", &edit.access_key_id), ("aws_secret_access_key", &edit.secret_access_key)]
        {
            if let Some(value) = value {
                credentials.set(&edit.name, key, Some(value));
                // A key in the config file would override the one written to credentials for the CLI.
                config.set(&header, key, None);
            }
        }
        write(&self.config, &config.render(), false)?;
        write(&self.credentials, &credentials.render(), true)
    }

    /// Removes profile `name`'s sections from both files.
    pub fn remove(&self, name: &str) -> Result<(), ProfileError> {
        let (mut config, mut credentials) = self.read()?;
        if !self.load()?.iter().any(|p| p.name == name) {
            return Err(ProfileError::NotFound(name.to_owned()));
        }
        config.remove_section(&config_header(name));
        credentials.remove_section(name);
        write(&self.config, &config.render(), false)?;
        write(&self.credentials, &credentials.render(), true)
    }
}

/// The profile the AWS CLI would use without `--profile`: `AWS_PROFILE`, else `default`.
pub fn selected_by_env() -> String {
    std::env::var("AWS_PROFILE").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "default".to_owned())
}

/// The config file's header for profile `name`: `default` stands alone, every other one is `profile NAME`.
fn config_header(name: &str) -> String {
    if name == "default" { name.to_owned() } else { format!("profile {name}") }
}

/// The profile a config file header names, or `None` for other sections (`services`, `sso-session`, ...).
fn profile_of_config_header(header: &str) -> Option<String> {
    if header == "default" {
        return Some(header.to_owned());
    }
    header.strip_prefix("profile ").map(|n| n.trim().to_owned())
}

/// Profile `name` from both documents; a value in the config file wins, as the SDKs merge them.
fn profile(config: &Document, credentials: &Document, name: String) -> Profile {
    let header = config_header(&name);
    let get = |key| config.get(&header, key).or_else(|| credentials.get(&name, key)).map(str::to_owned);
    let access_key_id = get("aws_access_key_id");
    let external = ["sso_session", "sso_start_url", "credential_process", "role_arn", "web_identity_token_file"];
    let credentials = if access_key_id.is_some() && get("aws_secret_access_key").is_some() {
        Credentials::Keys
    } else if config.keys(&header).iter().chain(credentials.keys(&name).iter()).any(|k| external.contains(k)) {
        Credentials::External
    } else {
        Credentials::Missing
    };
    Profile { endpoint_url: get("endpoint_url"), region: get("region"), access_key_id, credentials, name }
}

/// The document in `path`, empty when there's no such file.
fn read(path: &Path) -> Result<Document, ProfileError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Document::parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Document::default()),
        Err(source) => Err(ProfileError::Io { path: path.to_owned(), source }),
    }
}

/// Replaces `path` with `text` through a temporary file in its folder, so a crash leaves the old file or the new one,
/// never half of one. A `secret` file is made readable by its owner only, as the AWS CLI makes its credentials file;
/// a new folder is too.
fn write(path: &Path, text: &str, secret: bool) -> Result<(), ProfileError> {
    let io = |source| ProfileError::Io { path: path.to_owned(), source };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !dir.exists() {
        create_private_dir(dir).map_err(io)?;
    }
    let file_name = path.file_name().map_or_else(|| "aws".into(), |n| n.to_string_lossy());
    let tmp = dir.join(format!(".{file_name}.rs3manager-{}", std::process::id()));
    let result = (|| {
        let mut file = open_new(&tmp, secret || is_private(path))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// Creates `dir` and its parents, the last one readable by its owner only on Unix.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// Whether `path` exists with no access for group or others, so a rewrite keeps it that way.
#[cfg_attr(unix, expect(clippy::verbose_bit_mask, reason = "a permission mask reads as one"))]
fn is_private(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// A new file at `path`, owner-only on Unix when `private`.
fn open_new(path: &Path, private: bool) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files in a fresh temporary folder, with `config` and `credentials` as their text.
    fn files(config: &str, credentials: &str) -> (tempfile::TempDir, Files) {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::in_dir(dir.path());
        std::fs::write(&files.config, config).unwrap();
        std::fs::write(&files.credentials, credentials).unwrap();
        (dir, files)
    }

    /// A checked edit with every field.
    fn edit(name: &str, key: &str, secret: &str) -> ProfileEdit {
        ProfileEdit::new(name, "http://127.0.0.1:7480/", "", key, secret).unwrap()
    }

    #[test]
    fn lists_profiles_from_both_files_without_other_sections() {
        let (_dir, files) = files(
            "[default]\nregion = eu\n[profile sso]\nsso_session = corp\n[sso-session corp]\nsso_region = x\n",
            "[default]\naws_access_key_id = AK\naws_secret_access_key = SK\n[only-creds]\naws_access_key_id = B\n",
        );
        let profiles = files.load().unwrap();
        let summary: Vec<_> = profiles.iter().map(|p| (p.name.as_str(), p.credentials)).collect();
        assert_eq!(
            summary,
            [("default", Credentials::Keys), ("sso", Credentials::External), ("only-creds", Credentials::Missing)]
        );
        assert_eq!(profiles[0].region.as_deref(), Some("eu"));
        assert_eq!(profiles[0].access_key_id.as_deref(), Some("AK"));
    }

    #[test]
    fn missing_files_are_no_profiles_and_saving_creates_them_private() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::in_dir(&dir.path().join(".aws"));
        assert_eq!(files.load().unwrap(), []);
        files.save(None, &edit("dev", "AK", "SK")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&files.config).unwrap(),
            "[profile dev]\nendpoint_url = http://127.0.0.1:7480\nregion = default\n"
        );
        assert_eq!(
            std::fs::read_to_string(&files.credentials).unwrap(),
            "[dev]\naws_access_key_id = AK\naws_secret_access_key = SK\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&files.credentials), 0o600);
            assert_eq!(mode(&dir.path().join(".aws")), 0o700);
        }
    }

    #[test]
    fn editing_keeps_other_profiles_comments_and_the_secret_when_left_empty() {
        let config = "# team\n[profile a]\nregion = x\noutput = json\n\n[profile b]\nregion = y\n";
        let creds = "[a]\naws_access_key_id = OLD\naws_secret_access_key = KEEP\n\n[b]\naws_access_key_id = B\n";
        let (_dir, files) = files(config, creds);
        files.save(Some("a"), &edit("a", "NEW", "")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&files.config).unwrap(),
            "# team\n[profile a]\nregion = default\noutput = json\nendpoint_url = http://127.0.0.1:7480\n\n\
             [profile b]\nregion = y\n"
        );
        assert_eq!(
            std::fs::read_to_string(&files.credentials).unwrap(),
            "[a]\naws_access_key_id = NEW\naws_secret_access_key = KEEP\n\n[b]\naws_access_key_id = B\n"
        );
    }

    #[test]
    fn renaming_moves_both_sections_and_refuses_a_taken_name() {
        let (_dir, files) = files("[profile a]\nregion = x\n[profile b]\n", "[a]\naws_access_key_id = K\n");
        assert!(matches!(files.save(Some("a"), &edit("b", "", "")), Err(ProfileError::Exists(_))));
        files.save(Some("a"), &edit("c", "", "")).unwrap();
        let names: Vec<_> = files.load().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["c", "b"]);
        assert_eq!(std::fs::read_to_string(&files.credentials).unwrap(), "[c]\naws_access_key_id = K\n");
    }

    #[test]
    fn removing_drops_only_that_profile() {
        let (_dir, files) = files("[default]\nregion = x\n\n[profile a]\nregion = y\n", "[a]\nk = v\n[default]\n");
        files.remove("a").unwrap();
        assert_eq!(std::fs::read_to_string(&files.config).unwrap(), "[default]\nregion = x\n");
        assert_eq!(std::fs::read_to_string(&files.credentials).unwrap(), "[default]\n");
        assert!(matches!(files.remove("a"), Err(ProfileError::NotFound(_))));
    }

    #[test]
    fn typed_values_that_could_inject_sections_or_keys_are_refused() {
        let bad = [
            ProfileEdit::new("a]\n[evil", "http://h", "", "", ""),
            ProfileEdit::new("a", "http://h\naws_secret_access_key = x", "", "", ""),
            ProfileEdit::new("a", "ftp://h", "", "", ""),
            ProfileEdit::new("a", "http://", "", "", ""),
            ProfileEdit::new("a", "http://h", "us east", "", ""),
            ProfileEdit::new("a", "http://h", "", "AK\n[x]", ""),
            ProfileEdit::new("", "http://h", "", "", ""),
        ];
        assert!(bad.iter().all(Result::is_err), "{bad:?}");
        assert!(ProfileEdit::new("team-a.dev@x", "https://s3.example.com:8443", "eu-1", "AK", "s/+=").is_ok());
    }
}
