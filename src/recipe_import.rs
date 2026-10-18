//! Bounded, side-effect-free conversion of recipe sources to Fleet model fields.
//! Missing host choices stay visible in the preview rather than guessed or executed.
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchRecipeSource {
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImportedRecipeSource {
    pub source_url: String,
    pub source_format: String,
    pub source_content: String,
    pub suggested_id: String,
    pub named_volumes: Vec<String>,
    #[serde(default)]
    pub checkpoints: Vec<crate::dashboard_preparations::Checkpoint>,
}
/// Only the configurator's fixed HTTPS origin is fetched. Never follow a redirect
/// or accept an arbitrary download URL from remote HTML.
pub(crate) fn lil_source_url(text: &str) -> Result<url::Url, String> {
    if text.len() > 8192 {
        return Err("configuration URL exceeds 8 KiB".into());
    }
    let mut url = url::Url::parse(text).map_err(|_| "invalid configuration URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("docker.local-inference-lab.ai")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(url.path(), "/" | "/compose.yaml")
    {
        return Err("use an HTTPS configuration URL from docker.local-inference-lab.ai".into());
    }
    let pairs = url.query_pairs().collect::<Vec<_>>();
    for key in ["build", "model"] {
        if pairs
            .iter()
            .filter(|(k, v)| k == key && !v.is_empty())
            .count()
            != 1
        {
            return Err("configuration URL must select one build and model".into());
        }
    }
    if pairs.iter().any(|(key, value)| {
        credential(key)
            || regex::Regex::new(r"hf_[A-Za-z0-9]{16,}")
                .unwrap()
                .is_match(value)
    }) {
        return Err("configuration URL must not contain credentials".into());
    }
    url.set_fragment(None);
    url.set_path("/");
    Ok(url)
}
async fn source_text(response: reqwest::Response, limit: usize) -> Result<String, String> {
    if !response.status().is_success() {
        return Err("LiL source download did not succeed".into());
    };
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err("LiL source exceeds bounds".into());
    };
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "could not read LiL source")?;
        if bytes.len() + chunk.len() > limit {
            return Err("LiL source exceeds bounds".into());
        };
        bytes.extend_from_slice(&chunk)
    }
    String::from_utf8(bytes).map_err(|_| "source must be UTF-8".into())
}
fn decode_code(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest
            .find(';')
            .filter(|n| *n <= 12)
            .ok_or("invalid source HTML escape")?;
        let entity = &rest[..end];
        let ch = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = if let Some(hex) = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity.strip_prefix('#').and_then(|s| s.parse::<u32>().ok())
                };
                number
                    .and_then(char::from_u32)
                    .ok_or("invalid source HTML escape")?
            }
        };
        out.push(ch);
        rest = &rest[end + 1..]
    }
    out.push_str(rest);
    if out.len() > 32768 {
        return Err("source exceeds 32 KiB".into());
    };
    Ok(out)
}
fn page_code(page: &str, pane: &str) -> Result<String, String> {
    let needle = format!("id=\"{pane}\"");
    let start = page.find(&needle).ok_or("LiL page has no source panel")?;
    let tail = &page[start..];
    let code = tail
        .find("<pre><code>")
        .filter(|n| *n < 2048)
        .ok_or("LiL source panel is unavailable")?
        + "<pre><code>".len();
    let end = tail[code..]
        .find("</code>")
        .filter(|n| *n <= 65536)
        .ok_or("LiL source panel exceeds bounds")?;
    decode_code(&tail[code..code + end])
}
fn command_checkpoints(command: &str) -> Vec<crate::dashboard_preparations::Checkpoint> {
    use crate::dashboard_preparations::Checkpoint;
    let Ok(args) = shell_words::split(command) else {
        return vec![];
    };
    let option = |name: &str| {
        args.windows(2)
            .find(|p| p[0] == name)
            .map(|p| p[1].clone())
            .or_else(|| {
                args.iter()
                    .find_map(|s| s.strip_prefix(&format!("{name}=")).map(str::to_string))
            })
    };
    let mut found = Vec::new();
    if let Some(at) = args.iter().position(|s| s == "serve")
        && let (Some(model), Some(revision)) = (args.get(at + 1), option("--revision"))
    {
        let c = Checkpoint {
            repository: model.clone(),
            revision,
            subfolder: String::new(),
        };
        if c.valid() {
            found.push(c)
        }
    };
    if let Some(config) = option("--speculative-config")
        && let Ok(v) = serde_json::from_str::<Value>(&config)
        && let (Some(model), Some(revision)) = (v["model"].as_str(), v["revision"].as_str())
    {
        let c = Checkpoint {
            repository: model.into(),
            revision: revision.into(),
            subfolder: String::new(),
        };
        if c.valid() && !found.contains(&c) {
            found.push(c)
        }
    };
    found
}
fn imported_lil_page(url: &str, page: &str) -> Result<ImportedRecipeSource, String> {
    let compose = page_code(page, "pane-compose")?;
    let mut source = imported_lil_source(url, &compose)?;
    if let Ok(command) = page_code(page, "pane-vllm") {
        source.checkpoints = command_checkpoints(&command)
    };
    Ok(source)
}
pub(crate) async fn fetch_lil_source(text: &str) -> Result<ImportedRecipeSource, String> {
    let source_url = lil_source_url(text)?;
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 LLMConduit-RecipeImporter")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "source downloader unavailable")?;
    // The configurator can forbid /compose.yaml while publishing the same exact
    // source in its public page. Fetch one fixed origin; never execute page scripts.
    if let Ok(response) = client.get(source_url.clone()).send().await
        && let Ok(page) = source_text(response, 1024 * 1024).await
        && let Ok(source) = imported_lil_page(source_url.as_str(), &page)
    {
        return Ok(source);
    }
    let mut download = source_url.clone();
    download.set_path("/compose.yaml");
    let response = client
        .get(download)
        .send()
        .await
        .map_err(|_| "could not download LiL Compose source")?;
    let source = source_text(response, 32768).await?;
    imported_lil_source(source_url.as_str(), &source)
}
fn imported_lil_source(url: &str, content: &str) -> Result<ImportedRecipeSource, String> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|_| "source must be valid Compose YAML")?;
    if content.is_empty()
        || content.len() > 32 * 1024
        || crate::dashboard_management::source_has_credentials(&yaml)
        || regex::Regex::new(r"hf_[A-Za-z0-9]{16,}")
            .unwrap()
            .is_match(content)
    {
        return Err("source must be bounded and use host-local credential references".into());
    }
    let root = serde_json::to_value(yaml).map_err(|_| "invalid Compose keys")?;
    if !root.get("services").is_some_and(Value::is_object) {
        return Err("source must contain Compose services".into());
    }
    let named_volumes = root
        .get("volumes")
        .and_then(Value::as_object)
        .map(|v| v.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if named_volumes.len() > 32 || named_volumes.iter().any(|name| name.len() > 128) {
        return Err("too many or oversized named volumes".into());
    }
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("lil-import");
    let suggested_id = name
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(40)
        .collect();
    Ok(ImportedRecipeSource {
        source_url: url.into(),
        source_format: "compose".into(),
        source_content: content.into(),
        suggested_id,
        named_volumes,
        checkpoints: vec![],
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    #[serde(default)]
    pub checkpoints: Vec<crate::dashboard_preparations::Checkpoint>,
    pub source_url: Option<String>,
    /// Named Compose volume -> approved absolute host bind path.
    #[serde(default)]
    pub volume_paths: BTreeMap<String, String>,
    pub service: Option<String>,
    pub image: Option<String>,
    pub user: Option<String>,
    pub memory_limit: Option<String>,
    /// Explicit IDs, e.g. `device=0,1`; `all` needs conversion to an allocation.
    pub gpus: Option<String>,
    pub base_directory: Option<String>,
    pub home_directory: Option<String>,
    pub runtime_cache_directory: Option<String>,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub environment_files: Vec<String>,
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub readiness_url: Option<String>,
    pub num_nodes: Option<u32>,
    pub node_rank: Option<u32>,
    pub head_address: Option<String>,
    pub host_address: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertRecipeRequest {
    pub source_format: String,
    pub source_content: String,
    #[serde(default)]
    pub options: ImportOptions,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConvertedRecipe {
    pub source_format: String,
    pub services: Vec<String>,
    pub model_spec: Value,
    pub required_inputs: Vec<String>,
    pub warnings: Vec<String>,
}
struct Conversion<'a> {
    options: &'a ImportOptions,
    spec: Map<String, Value>,
    required: Vec<String>,
    warnings: Vec<String>,
    variables: BTreeMap<String, Value>,
}
fn credential(key: &str) -> bool {
    crate::dashboard_management::credential_key(key)
}
fn scalar(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(_) | Value::Bool(_) => Ok(value.to_string()),
        _ => Err("recipe values must be strings, numbers or booleans".into()),
    }
}
fn object(value: &Value) -> Result<&Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| "expected a YAML mapping".into())
}
fn array(value: &Value) -> Result<&Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| "expected a YAML list".into())
}
fn memory(value: &Value) -> Result<String, String> {
    let text = scalar(value)?.to_ascii_lowercase();
    let text = text
        .trim()
        .replace("gib", "g")
        .replace("mib", "m")
        .replace("kib", "k")
        .replace("gb", "g")
        .replace("mb", "m")
        .replace("kb", "k");
    let regex = regex::Regex::new(r"^[1-9][0-9]*[kmgtp]?$").unwrap();
    // Keep integer Docker units, with m/g normalized for host RAM admission.
    if !regex.is_match(&text) {
        return Err("memory and shm limits require a positive integer Docker size".into());
    }
    if text.ends_with('m') || text.ends_with('g') {
        return Ok(text);
    }
    let (number, multiplier) = match text.chars().last().unwrap() {
        'k' => (&text[..text.len() - 1], 1024u64),
        't' => (&text[..text.len() - 1], 1024u64.pow(4)),
        'p' => (&text[..text.len() - 1], 1024u64.pow(5)),
        _ => (text.as_str(), 1u64),
    };
    let bytes = number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or("size exceeds supported range")?;
    Ok(format!("{}m", bytes.div_ceil(1024 * 1024)))
}
impl<'a> Conversion<'a> {
    fn new(options: &'a ImportOptions) -> Self {
        let mut variables = options.variables.clone();
        if let Some(home) = &options.home_directory {
            variables.insert("HOME".into(), Value::String(home.clone()));
        }
        if let Some(base) = &options.base_directory {
            variables.insert("PWD".into(), Value::String(base.clone()));
        }
        Self {
            options,
            spec: Map::new(),
            required: vec![],
            warnings: vec![],
            variables,
        }
    }
    fn need(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.required.contains(&text) {
            self.required.push(text);
        }
    }
    fn warn(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.warnings.contains(&text) {
            self.warnings.push(text);
        }
    }
    fn expand(&mut self, text: &str) -> Result<String, String> {
        // Compose interpolation is data substitution only, never shell evaluation.
        let re=regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:(:-|-|:\?|\?|:\+|\+)([^}]*))?\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
        let mut rendered = text.replace("$$", "\u{1}");
        for _ in 0..10 {
            let mut failure = None;
            let next = re
                .replace_all(&rendered, |caps: &regex::Captures<'_>| {
                    let key = caps.get(1).or_else(|| caps.get(4)).unwrap().as_str();
                    let value = self.variables.get(key).and_then(|v| scalar(v).ok());
                    let operator = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    let fallback = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                    let set = value.is_some();
                    let nonempty = value.as_ref().is_some_and(|s| !s.is_empty());
                    match operator {
                        ":-" if !nonempty => fallback.into(),
                        "-" if !set => fallback.into(),
                        ":+" => {
                            if nonempty {
                                fallback.into()
                            } else {
                                String::new()
                            }
                        }
                        "+" => {
                            if set {
                                fallback.into()
                            } else {
                                String::new()
                            }
                        }
                        ":?" if !nonempty => {
                            self.need(format!("Set variable {key}"));
                            caps[0].into()
                        }
                        "?" if !set => {
                            self.need(format!("Set variable {key}"));
                            caps[0].into()
                        }
                        _ => value.unwrap_or_else(|| {
                            self.need(format!("Set variable {key}"));
                            caps[0].into()
                        }),
                    }
                })
                .into_owned();
            if next.len() > 32 * 1024 {
                failure = Some("expanded recipe exceeds 32 KiB".to_string());
            }
            if let Some(failure) = failure {
                return Err(failure);
            }
            if next == rendered {
                return Ok(next.replace('\u{1}', "$"));
            }
            rendered = next;
        }
        Err("variable expansion did not stabilize; remove cyclic references".into())
    }
    fn path(&mut self, text: &str) -> Result<String, String> {
        let text = self.expand(text)?;
        let text = if let Some(rest) = text.strip_prefix("~/") {
            match &self.options.home_directory {
                Some(home) => format!("{home}/{rest}"),
                None => {
                    self.need("Set the target host home directory");
                    text
                }
            }
        } else {
            text
        };
        let path = Path::new(&text);
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(base) = &self.options.base_directory {
            Path::new(base).join(path)
        } else {
            self.need("Set the absolute recipe directory on the target host");
            path.to_path_buf()
        };
        let mut result = std::path::PathBuf::new();
        for part in joined.components() {
            match part {
                Component::ParentDir => {
                    result.pop();
                }
                Component::CurDir => {}
                other => result.push(other.as_os_str()),
            }
        }
        if !result.is_absolute() {
            self.need("Host paths must be absolute")
        }
        Ok(result.to_string_lossy().into_owned())
    }
    fn args(&mut self, value: &Value) -> Result<Vec<String>, String> {
        match value {
            Value::String(s) => split_command(&self.expand(s)?),
            Value::Array(items) => items.iter().map(|v| self.expand(&scalar(v)?)).collect(),
            _ => Err("command and entrypoint must be a string or argument list".into()),
        }
    }
    fn environment(&mut self, value: Option<&Value>) -> Result<(), String> {
        let mut env = Map::new();
        let entries = match value {
            None => vec![],
            Some(Value::Object(map)) => map
                .iter()
                .map(|(k, v)| Ok((k.clone(), if v.is_null() { None } else { Some(scalar(v)?) })))
                .collect::<Result<Vec<_>, String>>()?,
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    let s = scalar(v)?;
                    Ok(match s.split_once('=') {
                        Some((k, v)) => (k.into(), Some(v.into())),
                        None => (s, None),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            _ => return Err("environment must be a mapping or KEY=value list".into()),
        };
        for (key, raw) in entries {
            if credential(&key) {
                if self.options.environment_files.is_empty()
                    && !self
                        .spec
                        .get("environment_files")
                        .and_then(Value::as_array)
                        .is_some_and(|v| !v.is_empty())
                {
                    if raw.is_none() {
                        self.warn(format!("Optional {key} passthrough is omitted; select a host-local environment file if authentication is required"));
                    } else {
                        self.need(format!("Provide a host-local environment file for {key}"));
                    }
                } else {
                    self.warn(format!(
                        "{key} is read only from the approved host-local environment file"
                    ));
                }
                continue;
            }
            let value = match raw {
                Some(value) => self.expand(&value)?,
                None => match self.variables.get(&key) {
                    Some(v) => scalar(v)?,
                    None => {
                        self.need(format!("Set variable {key}"));
                        continue;
                    }
                },
            };
            env.insert(key, Value::String(value));
        }
        for (key, value) in &self.options.environment {
            env.insert(key.clone(), Value::String(value.clone()));
        }
        self.spec.insert("environment".into(), Value::Object(env));
        Ok(())
    }
    fn common(&mut self) -> Result<(), String> {
        if let Some(image) = &self.options.image {
            self.spec.insert("image".into(), json!(image));
        }
        if let Some(user) = &self.options.user {
            self.spec.insert("user".into(), json!(user));
        }
        if let Some(limit) = &self.options.memory_limit {
            self.spec
                .insert("memory_limit".into(), json!(memory(&json!(limit))?));
        }
        if let Some(gpus) = &self.options.gpus {
            self.spec.insert("gpus".into(), json!(gpus));
        }
        for field in ["user", "memory_limit"] {
            if !self
                .spec
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            {
                self.need(format!("Set {field}"));
            }
        }
        if !self
            .spec
            .get("image")
            .and_then(Value::as_str)
            .is_some_and(pinned)
        {
            self.need("Select an immutable image: repository@sha256:digest or an existing host sha256:image-id");
        }
        if self.spec.get("gpus").and_then(Value::as_str) == Some("all") {
            let ids = self
                .spec
                .get("environment")
                .and_then(|e| e.get("CUDA_VISIBLE_DEVICES"))
                .and_then(Value::as_str)
                .filter(|s| {
                    s.split(',')
                        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                })
                .map(str::to_string);
            if let Some(ids) = ids {
                self.spec
                    .insert("gpus".into(), json!(format!("device={ids}")));
            } else {
                self.need("Select explicit GPU devices, e.g. device=0 or device=0,1");
            }
        }
        if self.spec.get("memory_limit").is_some() && !self.spec.contains_key("memory_swap_limit") {
            self.spec.insert(
                "memory_swap_limit".into(),
                self.spec["memory_limit"].clone(),
            );
            self.warn("Swap is limited to the RAM limit; no additional swap is allowed");
        }
        if let Some(files) = self.spec.get("environment_files").cloned() {
            let mut all = array(&files)?
                .iter()
                .map(scalar)
                .collect::<Result<Vec<_>, _>>()?;
            for file in &self.options.environment_files {
                let path = self.path(file)?;
                if !all.contains(&path) {
                    all.push(path)
                }
            }
            self.spec.insert("environment_files".into(), json!(all));
        } else if !self.options.environment_files.is_empty() {
            let files = self
                .options
                .environment_files
                .iter()
                .map(|s| self.path(s))
                .collect::<Result<Vec<_>, _>>()?;
            self.spec.insert("environment_files".into(), json!(files));
        }
        if !self.options.devices.is_empty() {
            let mut devices = self
                .spec
                .get("devices")
                .map(strings)
                .transpose()?
                .unwrap_or_default();
            for device in &self.options.devices {
                if !devices.contains(device) {
                    devices.push(device.clone());
                }
            }
            self.spec.insert("devices".into(), json!(devices));
        }
        if let Some(url) = &self.options.readiness_url {
            self.spec
                .insert("readiness".into(), json!({"url":url,"success_status":200}));
        }
        self.warn("Fleet checks the selected host's image, user, mount, device and memory policy before deployment");
        Ok(())
    }
}
fn pinned(image: &str) -> bool {
    image
        .rsplit_once("@sha256:")
        .map(|(_, d)| d)
        .or_else(|| image.strip_prefix("sha256:"))
        .is_some_and(|d| {
            d.len() == 64
                && d.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
// Compose's command string is an argument vector, not an implicit shell script.
fn split_command(text: &str) -> Result<Vec<String>, String> {
    let mut args = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut active = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {
                word.push(ch);
                active = true;
            }
            (_, '\\') => {
                let next = chars.next().ok_or("command ends with a backslash")?;
                if next != '\n' {
                    if quote == Some('"') && !matches!(next, '"' | '\\' | '$' | '`') {
                        word.push('\\')
                    }
                    word.push(next);
                    active = true;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                active = true;
            }
            (None, c) if c.is_whitespace() => {
                if active {
                    args.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            _ => {
                word.push(ch);
                active = true;
            }
        }
    }
    if quote.is_some() {
        return Err("command contains an unmatched quote".into());
    }
    if active {
        args.push(word)
    }
    Ok(args)
}
fn strings(value: &Value) -> Result<Vec<String>, String> {
    array(value)?.iter().map(scalar).collect()
}
fn ports(value: &Value) -> Result<Vec<Value>, String> {
    array(value)?
        .iter()
        .map(|item| {
            if let Value::Object(p) = item {
                if p.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "target"
                            | "published"
                            | "host_ip"
                            | "protocol"
                            | "mode"
                            | "name"
                            | "app_protocol"
                    )
                }) {
                    return Err("unsupported long port field".into());
                }
                let cp = p.get("target").ok_or("port requires target")?;
                let hp = p
                    .get("published")
                    .ok_or("port requires fixed published port")?;
                return port_object(
                    p.get("host_ip")
                        .map(scalar)
                        .transpose()?
                        .unwrap_or_else(|| "0.0.0.0".into()),
                    scalar(hp)?,
                    scalar(cp)?,
                    p.get("protocol")
                        .map(scalar)
                        .transpose()?
                        .unwrap_or_else(|| "tcp".into()),
                );
            }
            let text = scalar(item)?;
            let (numbers, proto) = text.split_once('/').unwrap_or((&text, "tcp"));
            let parts = numbers.rsplitn(3, ':').collect::<Vec<_>>();
            if parts.len() < 2 {
                return Err("ports require a fixed host:container mapping".into());
            }
            port_object(
                parts
                    .get(2)
                    .unwrap_or(&"0.0.0.0")
                    .trim_matches(['[', ']'])
                    .into(),
                parts[1].into(),
                parts[0].into(),
                proto.into(),
            )
        })
        .collect()
}
fn port_object(ip: String, hp: String, cp: String, proto: String) -> Result<Value, String> {
    let parse = |s: &str| {
        s.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| {
            "ports must be fixed integers from 1 to 65535; ranges are unsupported".to_string()
        })
    };
    if !matches!(proto.as_str(), "tcp" | "udp") {
        return Err("port protocol must be tcp or udp".into());
    }
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Err("published host_ip must be an IP address".into());
    }
    Ok(json!({"host_ip":ip,"host_port":parse(&hp)?,"container_port":parse(&cp)?,"protocol":proto}))
}
fn compose(c: &mut Conversion<'_>, root: &Value) -> Result<Vec<String>, String> {
    let root = object(root)?;
    for field in root.keys() {
        if !matches!(
            field.as_str(),
            "services" | "name" | "version" | "volumes" | "networks"
        ) && !field.starts_with("x-")
        {
            c.need(format!("Unsupported Compose root field: {field}"));
        }
    }
    let services = object(root.get("services").ok_or("Compose requires services")?)?;
    let names = services.keys().cloned().collect::<Vec<_>>();
    let selected = match &c.options.service {
        Some(name) => services
            .get(name)
            .ok_or("selected Compose service does not exist")?,
        None if services.len() == 1 => services.values().next().unwrap(),
        None => {
            c.need("Select one Compose service to convert");
            return Ok(names);
        }
    };
    let service = object(selected)?;
    let allowed = [
        "image",
        "command",
        "entrypoint",
        "environment",
        "env_file",
        "volumes",
        "ports",
        "gpus",
        "deploy",
        "mem_limit",
        "memswap_limit",
        "user",
        "ipc",
        "shm_size",
        "network_mode",
        "restart",
        "init",
        "privileged",
        "cap_add",
        "group_add",
        "devices",
        "security_opt",
        "ulimits",
        "read_only",
        "stop_grace_period",
        "healthcheck",
        "container_name",
        "hostname",
        "labels",
        "depends_on",
        "expose",
        "profiles",
    ];
    for field in service.keys() {
        if !allowed.contains(&field.as_str()) && !field.starts_with("x-") {
            c.need(format!("Unsupported Compose service field: {field}"));
        }
    }
    for field in [
        "container_name",
        "hostname",
        "labels",
        "depends_on",
        "profiles",
    ] {
        if service.contains_key(field) {
            c.warn(format!("Compose {field} is not carried into the managed container; service dependencies must be deployed separately"));
        }
    }
    for field in ["image", "user", "ipc", "network_mode", "restart"] {
        if let Some(v) = service.get(field) {
            let expanded = c.expand(&scalar(v)?)?;
            c.spec.insert(field.into(), json!(expanded));
        }
    }
    for field in ["init", "privileged", "read_only"] {
        if let Some(v) = service.get(field) {
            if !v.is_boolean() {
                return Err("Compose boolean field has an invalid type".into());
            }
            c.spec.insert(field.into(), v.clone());
        }
    }
    for field in ["shm_size", "mem_limit", "memswap_limit"] {
        if let Some(v) = service.get(field) {
            let key = match field {
                "mem_limit" => "memory_limit",
                "memswap_limit" => "memory_swap_limit",
                _ => field,
            };
            let size = c.expand(&scalar(v)?)?;
            c.spec.insert(key.into(), json!(memory(&json!(size))?));
        }
    }
    if let Some(v) = service.get("stop_grace_period") {
        c.spec.insert("stop_timeout".into(), json!(scalar(v)?));
    }
    if let Some(v) = service.get("env_file") {
        let items = if v.is_string() {
            vec![v.clone()]
        } else {
            array(v)?.clone()
        };
        let mut files = vec![];
        for v in items {
            if !v.is_string() {
                c.need("Long env_file entries need explicit required host-local files");
                continue;
            }
            files.push(c.path(&scalar(&v)?)?);
        }
        c.spec.insert("environment_files".into(), json!(files));
    }
    c.environment(service.get("environment"))?;
    if let Some(v) = service.get("command").filter(|v| !v.is_null()) {
        let args = c.args(v)?;
        if args.is_empty() {
            c.need("An empty Compose command cannot clear the image CMD in Fleet; set an explicit command");
        }
        c.spec.insert("command".into(), json!(args));
    }
    if let Some(v) = service.get("entrypoint").filter(|v| !v.is_null()) {
        let mut args = c.args(v)?;
        if args.is_empty() {
            c.need("An empty Compose entrypoint cannot be represented; set an explicit executable");
        } else {
            c.spec.insert("entrypoint".into(), json!(args.remove(0)));
            args.extend(
                c.spec
                    .get("command")
                    .map(strings)
                    .transpose()?
                    .unwrap_or_default(),
            );
            c.spec.insert("command".into(), json!(args));
        }
    }
    if let Some(v) = service.get("ports") {
        let mut expanded = vec![];
        for port in array(v)? {
            expanded.push(match port {
                Value::String(s) => json!(c.expand(s)?),
                Value::Object(map) => {
                    let mut map = map.clone();
                    for value in map.values_mut() {
                        if let Some(s) = value.as_str() {
                            *value = json!(c.expand(s)?);
                        }
                    }
                    Value::Object(map)
                }
                _ => port.clone(),
            })
        }
        c.spec
            .insert("ports".into(), json!(ports(&json!(expanded))?));
    }
    if let Some(v) = service.get("volumes") {
        let mut mounts = vec![];
        for volume in array(v)? {
            let (mut source, target, ro) = if let Value::Object(v) = volume {
                if !matches!(
                    v.get("type").and_then(Value::as_str),
                    Some("bind" | "volume")
                ) || v
                    .keys()
                    .any(|k| !matches!(k.as_str(), "type" | "source" | "target" | "read_only"))
                {
                    c.need("Only bind volumes without additional options can be converted");
                    continue;
                }
                (
                    scalar(v.get("source").ok_or("bind volume requires source")?)?,
                    scalar(v.get("target").ok_or("bind volume requires target")?)?,
                    v.get("read_only").and_then(Value::as_bool).unwrap_or(false),
                )
            } else {
                let text = c.expand(&scalar(volume)?)?;
                if text.contains("${") {
                    continue;
                }
                let parts = text.split(':').collect::<Vec<_>>();
                if parts.len() < 2 || parts.len() > 3 {
                    return Err("bind volumes require source:target[:ro|rw]".into());
                }
                if parts.len() == 3 && !matches!(parts[2], "ro" | "rw") {
                    c.need("Unsupported bind mount option; use an explicit supported mount");
                    continue;
                }
                (
                    parts[0].into(),
                    parts[1].into(),
                    parts.get(2) == Some(&"ro"),
                )
            };
            if !source.starts_with(['/', '.', '~', '$']) {
                if let Some(path) = c.options.volume_paths.get(&source) {
                    c.warn(format!("Named volume {source} uses the selected host bind path; existing Docker volume contents are not migrated"));
                    source = path.clone();
                } else {
                    c.need(format!("Set host bind path for named volume {source}"));
                    continue;
                }
            }
            if !target.starts_with('/') {
                return Err("container mount targets must be absolute".into());
            }
            let source = c.path(&source)?;
            mounts.push(json!({"source":source,"target":target,"read_only":ro}));
        }
        c.spec.insert("mounts".into(), json!(mounts));
    }
    for field in ["devices", "group_add", "cap_add", "security_opt"] {
        if let Some(v) = service.get(field) {
            c.spec.insert(field.into(), json!(strings(v)?));
        }
    }
    if let Some(v) = service.get("ulimits") {
        let mut limits = vec![];
        for (key, v) in object(v)? {
            if let Value::Object(v) = v {
                limits.push(format!(
                    "{key}={}:{}",
                    scalar(v.get("soft").ok_or("ulimit requires soft")?)?,
                    scalar(v.get("hard").ok_or("ulimit requires hard")?)?
                ));
            } else {
                limits.push(format!("{key}={}", scalar(v)?));
            }
        }
        c.spec.insert("ulimits".into(), json!(limits));
    }
    if let Some(v) = service.get("gpus") {
        if v.as_str() == Some("all") {
            c.spec.insert("gpus".into(), json!("all"));
        } else {
            c.need("GPU requests require an explicit device allocation");
        }
    }
    if let Some(v) = service.get("deploy") {
        let deploy = object(v)?;
        for field in deploy.keys() {
            if field != "resources" {
                c.need(format!("Unsupported Compose deploy field: {field}"));
            }
        }
        if let Some(resources) = deploy.get("resources") {
            let resources = object(resources)?;
            for field in resources.keys() {
                if !matches!(field.as_str(), "limits" | "reservations") {
                    c.need(format!("Unsupported Compose resource field: {field}"));
                }
            }
            if let Some(limits) = resources.get("limits") {
                let limits = object(limits)?;
                for field in limits.keys() {
                    if field != "memory" {
                        c.need(format!("Unsupported resource limit: {field}"));
                    }
                }
                if !c.spec.contains_key("memory_limit")
                    && let Some(v) = limits.get("memory")
                {
                    let size = c.expand(&scalar(v)?)?;
                    c.spec
                        .insert("memory_limit".into(), json!(memory(&json!(size))?));
                }
            }
            if let Some(reservations) = resources.get("reservations") {
                let reservations = object(reservations)?;
                for field in reservations.keys() {
                    if field != "devices" {
                        c.need(format!("Unsupported resource reservation: {field}"));
                    }
                }
                if let Some(v) = reservations.get("devices") {
                    let devices = array(v)?;
                    if devices.len() != 1 {
                        c.need("Convert one explicit NVIDIA GPU reservation per service");
                    } else {
                        let d = object(&devices[0])?;
                        if d.keys().any(|k| {
                            !matches!(
                                k.as_str(),
                                "driver" | "device_ids" | "capabilities" | "count"
                            )
                        }) {
                            c.need("Unsupported GPU reservation options");
                        }
                        if d.get("capabilities")
                            .map(strings)
                            .transpose()?
                            .is_some_and(|v| v.iter().any(|v| v != "gpu"))
                        {
                            c.need("Only GPU reservation capabilities are supported");
                        }
                        if let Some(count) = d.get("count")
                            && count.as_str() != Some("all")
                        {
                            c.need("Resolve GPU count to explicit device_ids");
                        }
                        if d.get("driver").and_then(Value::as_str) != Some("nvidia") {
                            c.need("Only NVIDIA GPU reservations are supported");
                        } else if let Some(ids) = d.get("device_ids") {
                            c.spec.insert(
                                "gpus".into(),
                                json!(format!("device={}", strings(ids)?.join(","))),
                            );
                        } else {
                            c.spec.insert("gpus".into(), json!("all"));
                        }
                    }
                }
            }
        }
    }
    if let Some(health) = service.get("healthcheck") {
        let health = object(health)?;
        if health.get("disable").and_then(Value::as_bool) == Some(true) {
            c.warn("Disabled healthcheck: Fleet readiness is container liveness");
        } else if let Some(test) = health.get("test") {
            let text = if test.is_array() {
                strings(test)?.join(" ")
            } else {
                scalar(test)?
            };
            // Healthchecks run inside Compose; Fleet probes the published host port.
            let env = c
                .spec
                .get("environment")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let original = c.variables.clone();
            for (key, v) in env {
                c.variables.entry(key).or_insert(v);
            }
            let text = c.expand(&text)?;
            c.variables = original;
            let url = regex::Regex::new(r#"https?://[^\s'\"|;]+"#)
                .unwrap()
                .find(&text)
                .map(|m| m.as_str().to_owned());
            if let Some(url) = url.filter(|_| c.options.readiness_url.is_none()) {
                c.spec.insert(
                    "readiness".into(),
                    json!({"url":host_probe(&url,&c.spec)?,"success_status":200}),
                );
            } else if c.options.readiness_url.is_none() {
                c.need("Provide readiness_url for the non-HTTP Compose healthcheck");
            }
            c.warn(
                "Compose healthcheck intervals/timeouts use the Fleet host's readiness settings",
            );
        }
    }
    Ok(names)
}
fn host_probe(url: &str, spec: &Map<String, Value>) -> Result<String, String> {
    let mut url = url::Url::parse(url).map_err(|_| "invalid healthcheck URL")?;
    if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(
            "healthcheck URL must address container loopback; override readiness_url explicitly"
                .into(),
        );
    }
    let port = url
        .port_or_known_default()
        .ok_or("healthcheck URL needs a port")?;
    let host_mode = spec.get("network_mode").and_then(Value::as_str) == Some("host");
    if !host_mode {
        let binding = spec
            .get("ports")
            .and_then(Value::as_array)
            .and_then(|p| {
                p.iter().find(|p| {
                    p["container_port"].as_u64() == Some(port.into()) && p["protocol"] == "tcp"
                })
            })
            .ok_or("HTTP healthcheck requires a published TCP port or host networking")?;
        url.set_port(binding["host_port"].as_u64().map(|p| p as u16))
            .map_err(|_| "invalid probe port")?;
    }
    url.set_host(Some("127.0.0.1"))
        .map_err(|_| "invalid probe host")?;
    Ok(url.to_string())
}
fn sparkrun(c: &mut Conversion<'_>, root: &Value) -> Result<(), String> {
    let root = object(root)?;
    let allowed = [
        "recipe_version",
        "runtime",
        "model",
        "model_revision",
        "container",
        "defaults",
        "env",
        "executor_config",
        "command",
        "pre_exec",
        "min_nodes",
        "max_nodes",
        "description",
        "metadata",
        "benchmark",
    ];
    for field in root.keys() {
        if !allowed.contains(&field.as_str()) {
            c.need(format!("Unsupported sparkrun field: {field}"));
        }
    }
    if !matches!(
        root.get("recipe_version")
            .map(scalar)
            .transpose()?
            .as_deref(),
        Some("1" | "2")
    ) {
        return Err("sparkrun recipe_version must be 1 or 2".into());
    }
    if root.get("runtime").and_then(Value::as_str) != Some("vllm") {
        c.need("Only explicit vLLM sparkrun command recipes are currently supported");
    }
    let mut values = root
        .get("defaults")
        .map(object)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    for field in ["model", "model_revision", "container"] {
        if let Some(v) = root.get(field) {
            values.entry(field).or_insert(v.clone());
        }
    }
    if let Some(model) = root.get("model") {
        values.entry("resolved_model_path").or_insert(model.clone());
    }
    values.extend(c.options.variables.clone());
    let mut env = c.variables.clone();
    env.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
    c.variables = env;
    if let Some(v) = root.get("container") {
        let image = c.expand(&scalar(v)?)?;
        c.spec.insert("image".into(), json!(image));
    }
    c.environment(root.get("env"))?;
    let source = root
        .get("command")
        .and_then(Value::as_str)
        .ok_or("sparkrun conversion requires an explicit command template")?;
    let mut command = render_spark(source, &values)?;
    let re = regex::Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    for caps in re.captures_iter(&command) {
        c.need(format!("Set sparkrun variable {}", &caps[1]));
    }
    let minimum = root.get("min_nodes").and_then(Value::as_u64).unwrap_or(1) as u32;
    let maximum = root
        .get("max_nodes")
        .and_then(Value::as_u64)
        .unwrap_or(minimum.into()) as u32;
    let nodes = c.options.num_nodes.unwrap_or(minimum);
    if nodes == 0 || nodes > 64 || nodes < minimum || nodes > maximum {
        return Err("node count is outside the recipe's supported range".into());
    }
    let mut headless = false;
    if nodes > 1 {
        let rank = c.options.node_rank;
        let head = c.options.head_address.as_deref();
        let host = c.options.host_address.as_deref();
        if rank.is_none() {
            c.need("Set node_rank for this distributed rank");
        }
        if head.is_none() {
            c.need("Set head_address for the distributed vLLM master");
        }
        if host.is_none() {
            c.need("Set host_address for this rank's VLLM_HOST_IP");
        }
        if let Some(rank) = rank {
            if rank >= nodes {
                return Err("node_rank must be less than num_nodes".into());
            }
            headless = rank > 0;
        }
        if let Some(head) = head
            && head != "{{head_address}}"
            && head.parse::<std::net::IpAddr>().is_err()
        {
            return Err("distributed head_address must be an IP address".into());
        }
        if let Some(host) = host {
            if host != "{{host_address}}" && host.parse::<std::net::IpAddr>().is_err() {
                return Err("distributed host_address must be an IP address".into());
            }
            c.spec
                .get_mut("environment")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("VLLM_HOST_IP".into(), json!(host));
        }
        if values
            .get("data_parallel")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            > 1
        {
            c.need(
                "Distributed data-parallel and hybrid layouts need explicit per-rank configuration",
            );
        }
        if command.contains("--nnodes")
            || command.contains("--node-rank")
            || command.contains("--distributed-executor-backend ray")
        {
            c.need("Preconfigured distributed/Ray commands require explicit conversion of their coordination setup");
        }
        if let (Some(rank), Some(head)) = (rank, head) {
            command = format!(
                "{} --nnodes {nodes} --node-rank {rank} --master-addr {head} --master-port 25000{}",
                command.trim_end(),
                if headless { " --headless" } else { "" }
            );
        }
        c.warn("This preview is one distributed rank; create a deployment per rank and preserve the fabric/NCCL environment. Rank startup is not atomic");
    }
    if let Some(hooks) = root.get("pre_exec") {
        let hooks = if hooks.is_string() {
            vec![scalar(hooks)?]
        } else {
            strings(hooks)?
        };
        let hooks = hooks
            .iter()
            .map(|s| render_spark(s, &values))
            .collect::<Result<Vec<_>, _>>()?;
        command = format!("{}\n{}", hooks.join("\n"), command);
        for caps in re.captures_iter(&command) {
            c.need(format!("Set sparkrun variable {}", &caps[1]));
        }
    }
    c.spec.insert("entrypoint".into(), json!("/bin/bash"));
    c.spec.insert("command".into(), json!(["-c", command]));
    c.spec.insert("network_mode".into(), json!("host"));
    c.spec.insert("ipc".into(), json!("host"));
    c.spec.insert("init".into(), json!(true));
    c.spec.insert("restart".into(), json!("unless-stopped"));
    c.spec.insert("gpus".into(), json!("device=0"));
    if let Some(executor) = root.get("executor_config") {
        for (field, value) in object(executor)? {
            match field.as_str() {
                "user" | "ipc" | "network_mode" => {
                    c.spec.insert(field.clone(), json!(scalar(value)?));
                }
                "shm_size" => {
                    c.spec.insert(field.clone(), json!(memory(value)?));
                }
                "security_opt" | "devices" | "group_add" | "cap_add" => {
                    c.spec.insert(field.clone(), json!(strings(value)?));
                }
                _ => c.need(format!("Unsupported sparkrun executor setting: {field}")),
            }
        }
    }
    if let Some(home) = &c.options.home_directory {
        let home = home.clone();
        let base = c
            .options
            .base_directory
            .clone()
            .unwrap_or_else(|| format!("{home}/compose"));
        let model = values
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("model");
        let directory: String = model
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .take(96)
            .collect();
        let cache = c
            .options
            .runtime_cache_directory
            .clone()
            .unwrap_or_else(|| format!("{base}/inference-cache/{directory}"));
        c.spec.insert("mounts".into(),json!([{"source":format!("{home}/.cache/huggingface"),"target":"/cache/huggingface"},{"source":cache,"target":"/cache/runtime"}]));
        let env = c
            .spec
            .get_mut("environment")
            .unwrap()
            .as_object_mut()
            .unwrap();
        for (k, v) in [
            ("HF_HOME", "/cache/huggingface"),
            ("HF_HUB_CACHE", "/cache/huggingface/hub"),
            ("XDG_CACHE_HOME", "/cache/runtime"),
            ("VLLM_CACHE_ROOT", "/cache/runtime/vllm"),
        ] {
            env.entry(k).or_insert(json!(v));
        }
        c.warn("Sparkrun cache binds are shown in the preview; prepare and approve these writable host paths before deployment");
    } else {
        c.need("Set the target host home directory for the model and runtime caches");
    }
    if !headless {
        if let Some(port) = values.get("port") {
            let port = scalar(port)?
                .parse::<u16>()
                .map_err(|_| "sparkrun port must be an integer")?;
            if port == 0 {
                return Err("sparkrun port must be nonzero".into());
            }
            c.spec.insert(
                "readiness".into(),
                json!({"url":format!("http://127.0.0.1:{port}/health"),"success_status":200}),
            );
        } else if c.options.readiness_url.is_none() {
            c.need("Set sparkrun port or readiness_url");
        }
    } else {
        c.warn("Headless worker readiness is container liveness; verify the complete world on its head rank");
    }
    Ok(())
}
fn render_spark(source: &str, values: &Map<String, Value>) -> Result<String, String> {
    let escaped = source.contains("{{");
    let re = regex::Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    let mut rendered = String::new();
    let mut position = 0;
    while position < source.len() {
        let tail = &source[position..];
        if escaped && tail.starts_with("{{") {
            rendered.push('\u{0}');
            position += 2;
        } else if let Some(found) = re.find(tail).filter(|m| m.start() == 0) {
            rendered.push_str(found.as_str());
            position += found.end();
        } else if escaped && tail.starts_with("}}") {
            rendered.push('\u{1}');
            position += 2;
        } else {
            let ch = tail.chars().next().unwrap();
            rendered.push(ch);
            position += ch.len_utf8();
        }
    }
    for _ in 0..10 {
        let next = re
            .replace_all(&rendered, |caps: &regex::Captures<'_>| {
                values
                    .get(&caps[1])
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    })
                    .unwrap_or_else(|| caps[0].into())
            })
            .into_owned();
        if next.len() > 32 * 1024 {
            return Err("rendered sparkrun command exceeds 32 KiB".into());
        }
        if next == rendered {
            return Ok(next.replace('\u{0}', "{").replace('\u{1}', "}"));
        }
        rendered = next;
    }
    Err("sparkrun placeholders did not stabilize; remove cyclic references".into())
}
pub fn convert(body: &ConvertRecipeRequest) -> Result<ConvertedRecipe, String> {
    if body.options.checkpoints.len() > 8 || body.options.checkpoints.iter().any(|c| !c.valid()) {
        return Err("checkpoint requirements must pin immutable revisions".into());
    };
    if body.source_content.is_empty() || body.source_content.len() > 32 * 1024 {
        return Err("source content must be nonempty and at most 32 KiB".into());
    }
    if serde_json::to_vec(&body.options)
        .map_err(|_| "invalid import options")?
        .len()
        > 16 * 1024
    {
        return Err("conversion options exceed 16 KiB".into());
    }
    if body.options.volume_paths.len() > 32
        || body.options.volume_paths.iter().any(|(name, path)| {
            name.is_empty()
                || name.len() > 128
                || path.len() > 4096
                || !Path::new(path).is_absolute()
        })
    {
        return Err("volume mappings require bounded names and absolute host paths".into());
    }
    if let Some(url) = &body.options.source_url {
        lil_source_url(url)?;
    }
    let mut yaml: serde_yaml::Value =
        serde_yaml::from_str(&body.source_content).map_err(|_| "source must be valid YAML")?;
    if crate::dashboard_management::source_has_credentials(&yaml)
        || regex::Regex::new(r"hf_[A-Za-z0-9]{16,}")
            .unwrap()
            .is_match(&body.source_content)
    {
        return Err("source credentials must use host-local references".into());
    }
    if body
        .options
        .variables
        .keys()
        .chain(body.options.environment.keys())
        .any(|k| credential(k))
    {
        return Err(
            "credentials must come from host-local environment files, never conversion variables"
                .into(),
        );
    }
    for path in [
        &body.options.base_directory,
        &body.options.home_directory,
        &body.options.runtime_cache_directory,
    ]
    .into_iter()
    .flatten()
    {
        if !Path::new(path).is_absolute() {
            return Err("conversion directories must be absolute host paths".into());
        }
    }
    yaml.apply_merge().map_err(|_| "invalid YAML merge key")?;
    let option_yaml =
        serde_yaml::to_value(&body.options.variables).map_err(|_| "invalid variables")?;
    let option_bytes = serde_json::to_string(&body.options).map_err(|_| "invalid options")?;
    if crate::dashboard_management::source_has_credentials(&option_yaml)
        || regex::Regex::new(r"hf_[A-Za-z0-9]{16,}")
            .unwrap()
            .is_match(&option_bytes)
    {
        return Err("conversion options must not contain inline credentials".into());
    }
    let source = serde_json::to_value(yaml).map_err(|_| "YAML must contain string mapping keys")?;
    let mut c = Conversion::new(&body.options);
    let services = match body.source_format.as_str() {
        "compose" => compose(&mut c, &source)?,
        "sparkrun" => {
            sparkrun(&mut c, &source)?;
            vec![]
        }
        _ => return Err("source_format must be compose or sparkrun".into()),
    };
    c.common()?;
    if let Some(url) = &body.options.source_url {
        let mut meta = json!({"source_url": lil_source_url(url)?.as_str()});
        if let Some(image) = c.spec.get("image").and_then(Value::as_str) {
            meta["image_source_urls"] = json!({image: "https://docker.local-inference-lab.ai/"});
        }
        if c.spec.get("entrypoint").and_then(Value::as_str) == Some("/usr/local/bin/lil-serve") {
            let args = c
                .spec
                .get("command")
                .map(strings)
                .transpose()?
                .unwrap_or_default();
            if args
                .windows(2)
                .any(|p| p[0] == "--hardware" && p[1] == "rtx-pro-6000-pcie")
            {
                meta["gpu_families"] = json!(["rtx-pro-6000-blackwell"]);
            }
        }
        c.spec.insert("metadata".into(), meta);
    }
    if !body.options.checkpoints.is_empty() {
        let meta = c.spec.entry("metadata").or_insert_with(|| json!({}));
        meta["checkpoints"] = json!(body.options.checkpoints);
    }
    let spec = Value::Object(c.spec);
    if serde_json::to_vec(&spec)
        .map_err(|_| "invalid converted recipe")?
        .len()
        > 32 * 1024
    {
        return Err("converted specification exceeds 32 KiB".into());
    }
    Ok(ConvertedRecipe {
        source_format: body.source_format.clone(),
        services,
        model_spec: spec,
        required_inputs: c.required,
        warnings: c.warnings,
    })
}

/// Convert distributed sources into host-independent rank templates in one revision.
pub fn convert_portable(body: &ConvertRecipeRequest) -> Result<ConvertedRecipe, String> {
    let minimum = if body.source_format == "sparkrun" {
        serde_yaml::from_str::<serde_json::Value>(&body.source_content)
            .ok()
            .and_then(|v| v.get("min_nodes").and_then(Value::as_u64))
            .unwrap_or(1)
    } else {
        1
    };
    let count = body.options.num_nodes.map(u64::from).unwrap_or(minimum);
    if count <= 1 || body.options.node_rank.is_some() {
        return convert(body);
    }
    if count > 8 {
        return Err("distributed recipes support at most eight ranks".into());
    }
    let mut request = body.clone();
    request.options.num_nodes = Some(count as u32);
    request.options.head_address = Some("{{head_address}}".into());
    request.options.host_address = Some("{{host_address}}".into());
    let mut ranks = Vec::new();
    let mut combined: Option<ConvertedRecipe> = None;
    for rank in 0..count {
        request.options.node_rank = Some(rank as u32);
        let result = convert(&request)?;
        ranks.push(result.model_spec.clone());
        if let Some(combined) = combined.as_mut() {
            for input in result.required_inputs {
                if !combined.required_inputs.contains(&input) {
                    combined.required_inputs.push(input);
                }
            }
            for warning in result.warnings {
                if !combined.warnings.contains(&warning) {
                    combined.warnings.push(warning);
                }
            }
        } else {
            combined = Some(result);
        }
    }
    let mut combined = combined.unwrap();
    let metadata = combined.model_spec.get("metadata").cloned();
    combined.model_spec = json!({"distributed":ranks});
    if let Some(metadata) = metadata {
        combined.model_spec["metadata"] = metadata;
    }

    combined
        .warnings
        .retain(|w| !w.starts_with("This preview is one distributed rank"));
    combined.warnings.push("Choose ordered hosts and their fabric addresses at deployment; rank 0 is the head. Fleet admits each rank independently; partial startup is reported per host.".into());
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lil_urls_keep_build_model_and_overrides_but_restrict_the_fetch_origin() {
        let url = lil_source_url("https://docker.local-inference-lab.ai/?build=pinned&model=qwen38-flash-next&o.port=8123&preset=#step-configure").unwrap();
        assert_eq!(url.fragment(), None);
        assert!(url.as_str().contains("o.port=8123"));
        for url in [
            "http://docker.local-inference-lab.ai/?build=x&model=x",
            "https://localhost/?build=x&model=x",
            "https://docker.local-inference-lab.ai.evil.test/?build=x&model=x",
            "https://secret@docker.local-inference-lab.ai/?build=x&model=x",
            "https://docker.local-inference-lab.ai:444/?build=x&model=x",
            "https://docker.local-inference-lab.ai/redirect?build=x&model=x",
            "https://docker.local-inference-lab.ai/?model=x",
            "https://docker.local-inference-lab.ai/?build=x&build=y&model=x",
            "https://docker.local-inference-lab.ai/?build=x&model=x&e.HF_TOKEN=private",
        ] {
            assert!(lil_source_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn lil_source_and_named_cache_bindings_preserve_the_pinned_launch() {
        let source = "name: lil-qwen\nservices:\n  model:\n    image: ghcr.io/local-inference-lab/vllm@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n    entrypoint: [/usr/local/bin/lil-serve]\n    command: [--profile, qwen38-flash-next, --hardware, rtx-pro-6000-pcie, --, --cache-mode, vram]\n    volumes: [hf:/root/.cache/huggingface, {type: volume, source: runtime, target: /cache, read_only: false}]\nvolumes: {hf: {}, runtime: {}}\n";
        let url = "https://docker.local-inference-lab.ai/?build=pinned&model=qwen38-flash-next";
        let loaded = imported_lil_source(url, source).unwrap();
        assert_eq!(loaded.named_volumes, ["hf", "runtime"]);
        assert_eq!(loaded.suggested_id, "lil-qwen");
        let value = serde_json::to_value(&loaded).unwrap();
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ImportedRecipeSource>(value.clone()).unwrap()
            )
            .unwrap(),
            value
        );
        let fetch = FetchRecipeSource { url: url.into() };
        let value = serde_json::to_value(&fetch).unwrap();
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<FetchRecipeSource>(value.clone()).unwrap()
            )
            .unwrap(),
            value
        );
        let mut body = request("compose", source);
        body.options.image = None;
        let unresolved = convert(&body).unwrap();
        assert!(
            unresolved
                .required_inputs
                .iter()
                .any(|m| m.contains("named volume hf"))
        );
        body.options.source_url = Some(url.into());
        body.options.volume_paths = BTreeMap::from([
            ("hf".into(), "/home/koen/.cache/huggingface".into()),
            (
                "runtime".into(),
                "/home/koen/compose/inference-cache/qwen".into(),
            ),
        ]);
        let converted = convert(&body).unwrap();
        assert!(
            converted.required_inputs.is_empty(),
            "{:?}",
            converted.required_inputs
        );
        assert_eq!(converted.model_spec["command"][0], "--profile");
        assert_eq!(converted.model_spec["mounts"][1]["target"], "/cache");
        assert_eq!(converted.model_spec["metadata"]["source_url"], url);
        assert_eq!(
            converted.model_spec["metadata"]["gpu_families"][0],
            "rtx-pro-6000-blackwell"
        );
        crate::dashboard_management::validate_recipe(
            &crate::dashboard_management::CreateRecipeRevision {
                description: String::new(),
                model_spec: converted.model_spec,
            },
        )
        .unwrap();
        let value = serde_json::to_value(&body.options).unwrap();
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<ImportOptions>(value.clone()).unwrap())
                .unwrap(),
            value
        );
        body.options
            .volume_paths
            .insert("hf".into(), "relative/path".into());
        assert!(convert(&body).is_err());
        assert!(
            imported_lil_source(url, "services: {model: {environment: {HF_TOKEN: private}}}")
                .is_err()
        );
        assert!(imported_lil_source(url, "<html>no Compose</html>").is_err());
    }
    #[test]
    fn optional_secret_passthrough_never_reads_controller_credentials() {
        let optional = convert(&request(
            "compose",
            "services: {model: {environment: [HF_TOKEN]}}",
        ))
        .unwrap();
        assert!(optional.required_inputs.is_empty());
        assert!(optional.model_spec["environment"].get("HF_TOKEN").is_none());
        assert!(
            optional
                .warnings
                .iter()
                .any(|s| s.contains("Optional HF_TOKEN"))
        );
        let required = convert(&request(
            "compose",
            "services: {model: {environment: {HF_TOKEN: '${HF_TOKEN}'}}}",
        ))
        .unwrap();
        assert!(
            required
                .required_inputs
                .iter()
                .any(|s| s.contains("host-local environment file"))
        );
    }
    fn request(format: &str, source: &str) -> ConvertRecipeRequest {
        ConvertRecipeRequest {
            source_format: format.into(),
            source_content: source.into(),
            options: ImportOptions {
                image: Some(format!("test@sha256:{}", "a".repeat(64))),
                user: Some("1000:1000".into()),
                memory_limit: Some("116000m".into()),
                home_directory: Some("/home/koen".into()),
                base_directory: Some("/home/koen/compose/test".into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn compose_null_defaults_and_empty_overrides_are_distinct() {
        let defaults = convert(&request(
            "compose",
            "services:\n  app:\n    command: null\n    entrypoint: null\n",
        ))
        .unwrap();
        assert!(
            !defaults
                .model_spec
                .as_object()
                .unwrap()
                .contains_key("command")
        );
        let empty = convert(&request(
            "compose",
            "services:\n  app:\n    command: []\n    entrypoint: []\n",
        ))
        .unwrap();
        assert_eq!(empty.required_inputs.len(), 2);
    }
    #[test]
    fn interpolated_limits_and_long_ports_preserve_values() {
        let mut body = request(
            "compose",
            "services:\n  app:\n    mem_limit: '${RAM:-64g}'\n    ports:\n      - target: '${PORT:-8000}'\n        published: '${HOST_PORT:-8001}'\n",
        );
        body.options.memory_limit = None;
        let result = convert(&body).unwrap();
        assert!(result.required_inputs.is_empty());
        assert_eq!(result.model_spec["memory_limit"], "64g");
        assert_eq!(result.model_spec["ports"][0]["host_port"], 8001);
    }
    #[test]
    fn compose_preserves_real_gemma_command_ports_devices_and_probe() {
        let body = request(
            "compose",
            r#"
services:
  gemma:
    image: vllm/vllm-openai:v0.26.0
    ipc: host
    shm_size: 16gb
    ports: ["8001:8000"]
    volumes: [/hf:/hf]
    environment: [NCCL_IB_DISABLE=1, HF_HOME=/hf]
    command: >-
      nvidia/Gemma-4-31B-IT-NVFP4
      --served-model-name Gemma-4-31B-it
      --limit-mm-per-prompt '{"image":20,"audio":0,"video":0}'
      --speculative-config '{"model":"assistant","num_speculative_tokens":4}'
      --enable-prefix-caching --enable-prompt-tokens-details
      --port 8000
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              device_ids: ["0"]
              capabilities: [gpu]
    healthcheck:
      test: [CMD, curl, -fsS, http://localhost:8000/health]
"#,
        );
        let p = convert(&body).unwrap();
        assert!(p.required_inputs.is_empty(), "{:?}", p.required_inputs);
        assert_eq!(p.model_spec["shm_size"], "16g");
        assert_eq!(p.model_spec["gpus"], "device=0");
        assert_eq!(p.model_spec["ports"][0]["host_port"], 8001);
        assert_eq!(
            p.model_spec["readiness"]["url"],
            "http://127.0.0.1:8001/health"
        );
        assert_eq!(
            p.model_spec["command"][4],
            r#"{"image":20,"audio":0,"video":0}"#
        );
        assert!(p.model_spec.get("entrypoint").is_none());
        assert_eq!(p.model_spec["environment"]["HF_HOME"], "/hf");
    }
    #[test]
    fn compose_environment_defaults_secret_files_and_relative_binds() {
        let mut body = request(
            "compose",
            r#"
services:
  glm:
    image: ${IMAGE:-localinferencelab/vllm:latest}
    network_mode: host
    privileged: true
    gpus: all
    environment:
      CUDA_VISIBLE_DEVICES: ${GPU_DEVICES:-0,1,2,3}
      HF_TOKEN: ${HF_TOKEN:?set host-local token}
      MAX_NUM_BATCHED_TOKENS: ${BATCH:-4096}
      PORT: ${PORT:-8000}
    volumes: ["${CONTAINER_TMP:-./cache/tmp}:/container-tmp"]
    healthcheck:
      test: [CMD-SHELL, "curl -fsS http://127.0.0.1:${PORT:-8000}/health || exit 1"]
"#,
        );
        body.options.environment_files =
            vec!["/home/koen/compose/fleet-observer/.secrets/inference.env".into()];
        let p = convert(&body).unwrap();
        assert!(p.required_inputs.is_empty(), "{:?}", p.required_inputs);
        assert_eq!(p.model_spec["gpus"], "device=0,1,2,3");
        assert_eq!(
            p.model_spec["environment"]["MAX_NUM_BATCHED_TOKENS"],
            "4096"
        );
        assert!(p.model_spec["environment"].get("HF_TOKEN").is_none());
        assert_eq!(
            p.model_spec["mounts"][0]["source"],
            "/home/koen/compose/test/cache/tmp"
        );
        assert_eq!(
            p.model_spec["readiness"]["url"],
            "http://127.0.0.1:8000/health"
        );
    }
    #[test]
    fn compose_service_selection_does_not_start_dependencies() {
        let mut b = request(
            "compose",
            "services: {one: {image: a}, two: {image: b, depends_on: [one]}}",
        );
        let p = convert(&b).unwrap();
        assert_eq!(p.services, vec!["one", "two"]);
        assert!(!p.required_inputs.is_empty());
        b.options.service = Some("two".into());
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("dependencies")));
    }
    #[test]
    fn mutable_images_and_missing_memory_are_visible_not_deployable() {
        let mut b = request("compose", "services: {test: {image: ubuntu:latest}}");
        b.options.image = None;
        b.options.memory_limit = None;
        b.options.user = None;
        let p = convert(&b).unwrap();
        for need in ["immutable image", "memory_limit", "user"] {
            assert!(p.required_inputs.iter().any(|s| s.contains(need)))
        }
    }
    #[test]
    fn unsupported_features_are_blocking_not_silently_dropped() {
        let p=convert(&request("compose","services: {x: {image: x, build: ., volumes: [cache:/cache], tmpfs: [/tmp], deploy: {replicas: 2}}}")).unwrap();
        for field in ["build", "tmpfs", "named volume cache", "replicas"] {
            assert!(
                p.required_inputs.iter().any(|s| s.contains(field)),
                "{:?}",
                p.required_inputs
            )
        }
    }
    #[test]
    fn entrypoint_array_does_not_lose_its_arguments() {
        let p = convert(&request(
            "compose",
            r#"services: {x: {image: x, entrypoint: [bash, -c], command: ["echo hello"]}}"#,
        ))
        .unwrap();
        assert_eq!(p.model_spec["entrypoint"], "bash");
        assert_eq!(p.model_spec["command"], json!(["-c", "echo hello"]));
    }
    #[test]
    fn interpolation_is_data_only_and_missing_values_are_reported() {
        let mut b = request(
            "compose",
            r#"services: {x: {image: x, environment: {LITERAL: "$$(uname)", VALUE: "${VALUE}", OPTIONAL: "${EMPTY:-fallback}"}}}"#,
        );
        b.options.variables.insert("EMPTY".into(), json!(""));
        let p = convert(&b).unwrap();
        assert_eq!(p.model_spec["environment"]["LITERAL"], "$(uname)");
        assert_eq!(p.model_spec["environment"]["OPTIONAL"], "fallback");
        assert!(p.required_inputs.contains(&"Set variable VALUE".into()));
    }
    #[test]
    fn credentials_are_never_accepted_as_conversion_values() {
        for content in [
            "services: {x: {image: x, environment: {HF_TOKEN: actual-secret}}}",
            "services: {x: {image: x, environment: [PASSWORD=literal]}}",
        ] {
            assert!(convert(&request("compose", content)).is_err());
        }
        let mut b = request("compose", "services: {x: {image: x}}");
        b.options
            .variables
            .insert("HF_TOKEN".into(), json!("secret"));
        assert!(convert(&b).is_err());
    }
    #[test]
    fn sparkrun_nested_json_templates_keep_flags_and_environment() {
        let b = request(
            "sparkrun",
            r#"
recipe_version: '2'
runtime: vllm
model: repo/model
container: vllm-node
defaults: {port: 8101, tensor_parallel: 1, num_speculative_tokens: 3, max_num_batched_tokens: 8192}
env: {B12X_POLICY_MODE: auto, NUM_TOKENS: 4}
executor_config: {security_opt: [no-new-privileges, seccomp=unconfined], user: '0:0'}
command: >-
  vllm serve {model} --port {port} -tp {tensor_parallel}
  --speculative-config '{"method":"mtp","num_speculative_tokens":{num_speculative_tokens}}'
  --compilation-config '{"pass_config":{"fuse_act_quant":true}}'
  --max-num-batched-tokens {max_num_batched_tokens} --enable-prefix-caching
"#,
        );
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.is_empty(), "{:?}", p.required_inputs);
        let cmd = p.model_spec["command"][1].as_str().unwrap();
        assert!(cmd.contains("--port 8101"));
        assert!(cmd.contains(r#"{"method":"mtp","num_speculative_tokens":3}"#));
        assert!(cmd.contains(r#"{"pass_config":{"fuse_act_quant":true}}"#));
        assert_eq!(p.model_spec["environment"]["B12X_POLICY_MODE"], "auto");
        assert_eq!(p.model_spec["environment"]["NUM_TOKENS"], "4");
        assert_eq!(
            p.model_spec["readiness"]["url"],
            "http://127.0.0.1:8101/health"
        );
    }
    #[test]
    fn sparkrun_v1_escaped_json_and_variable_cycles() {
        let mut b = request(
            "sparkrun",
            "recipe_version: '1'\nruntime: vllm\nmodel: repo/model\ncontainer: image\ndefaults: {port: 8000, count: 3}\ncommand: vllm serve {model} --speculative-config '{{\"n\":{count}}}'\n",
        );
        let p = convert(&b).unwrap();
        assert!(
            p.model_spec["command"][1]
                .as_str()
                .unwrap()
                .contains(r#"'{"n":3}'"#)
        );
        b.options.variables.insert("model".into(), json!("{model}"));
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.iter().any(|s| s.contains("model")));
    }

    #[test]
    fn portable_sparkrun_conversion_produces_all_ranks_without_host_addresses() {
        let b = request(
            "sparkrun",
            "recipe_version: '2'\nruntime: vllm\nmodel: repo/model\ncontainer: image\nmin_nodes: 2\nmax_nodes: 2\ndefaults: {port: 8131, tensor_parallel: 2}\ncommand: vllm serve {model} --port {port} -tp {tensor_parallel}\n",
        );
        let preview = convert_portable(&b).unwrap();
        assert!(
            preview.required_inputs.is_empty(),
            "{:?}",
            preview.required_inputs
        );
        assert_eq!(
            preview.model_spec["distributed"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            preview.model_spec["distributed"][1]["environment"]["VLLM_HOST_IP"],
            "{{host_address}}"
        );
        assert!(
            preview.model_spec["distributed"][1]["command"][1]
                .as_str()
                .unwrap()
                .contains("--headless")
        );
    }
    #[test]
    fn distributed_sparkrun_requires_rank_and_fabric_addresses() {
        let mut b = request(
            "sparkrun",
            "recipe_version: '2'\nruntime: vllm\nmodel: repo/model\ncontainer: image\nmin_nodes: 2\nmax_nodes: 2\ndefaults: {port: 8131, tensor_parallel: 2}\ncommand: vllm serve {model} --port {port} -tp {tensor_parallel}\n",
        );
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.iter().any(|s| s.contains("node_rank")));
        b.options.node_rank = Some(1);
        b.options.head_address = Some("192.168.37.107".into());
        b.options.host_address = Some("192.168.37.108".into());
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.is_empty(), "{:?}", p.required_inputs);
        assert!(p.model_spec["command"][1].as_str().unwrap().contains(
            "--nnodes 2 --node-rank 1 --master-addr 192.168.37.107 --master-port 25000 --headless"
        ));
        assert!(p.model_spec.get("readiness").is_none());
        assert_eq!(
            p.model_spec["environment"]["VLLM_HOST_IP"],
            "192.168.37.108"
        );
    }
    #[test]
    fn size_and_schema_limits_reject_invalid_inputs() {
        let mut b = request("compose", "services: {x: {image: x}}");
        b.source_content = "a".repeat(32769);
        assert!(convert(&b).is_err());
        for s in [
            "services: [",
            "services: {x: {image: x, ports: ['8000-8002:8000-8002']}}",
            "services: {x: {image: x, command: \"'unterminated\"}}",
        ] {
            assert!(convert(&request("compose", s)).is_err());
        }
        assert_eq!(memory(&json!("16gb")).unwrap(), "16g");
        assert_eq!(memory(&json!(1073741824)).unwrap(), "1024m");
        assert!(memory(&json!("-1")).is_err());
    }
    #[test]
    fn compose_anchors_and_source_secret_references() {
        let b = request(
            "compose",
            "x-defaults: &defaults {image: tagged, user: '1000:1000', mem_limit: 256m}\nservices: {x: {<<: *defaults, env_file: /private/env, environment: {HF_TOKEN: '${HF_TOKEN}'}}}\n",
        );
        let p = convert(&b).unwrap();
        assert!(p.required_inputs.is_empty(), "{:?}", p.required_inputs);
        assert_eq!(p.model_spec["environment_files"], json!(["/private/env"]));
        for secret in [
            "prefix${PASSWORD}",
            "${PASSWORD:-literal}",
            "!custom literal",
        ] {
            let source = format!(
                "services: {{x: {{image: tagged, environment: {{PASSWORD: '{secret}'}}}}}}"
            );
            assert!(convert(&request("compose", &source)).is_err());
        }
    }
    #[test]
    fn source_cache_and_device_overrides_are_explicit_and_bounded() {
        let mut b = request(
            "sparkrun",
            "recipe_version: '2'\nruntime: vllm\nmodel: repo/model\ncontainer: image\ndefaults: {port: 8000}\ncommand: vllm serve {model}\n",
        );
        b.options.runtime_cache_directory =
            Some("/home/koen/compose/inference-cache/existing".into());
        b.options.devices = vec!["/dev/infiniband:/dev/infiniband:rwm".into()];
        let p = convert(&b).unwrap();
        assert_eq!(
            p.model_spec["mounts"][1]["source"],
            "/home/koen/compose/inference-cache/existing"
        );
        assert_eq!(
            p.model_spec["devices"],
            json!(["/dev/infiniband:/dev/infiniband:rwm"])
        );
        b.options.runtime_cache_directory = Some("relative".into());
        assert!(convert(&b).is_err());
    }
    #[test]
    fn new_conversion_wire_fields_round_trip() {
        let mut b = request("sparkrun", "source");
        b.options.service = Some("svc".into());
        b.options.gpus = Some("device=0,1".into());
        b.options.environment_files = vec!["/secrets/env".into()];
        b.options.runtime_cache_directory = Some("/cache/existing".into());
        b.options.devices = vec!["/dev/infiniband:/dev/infiniband:rwm".into()];
        b.options.variables.insert("port".into(), json!(8131));
        b.options
            .environment
            .insert("NCCL_IB_HCA".into(), "rocep1s0f1".into());
        b.options.readiness_url = Some("http://127.0.0.1:8131/health".into());
        b.options.num_nodes = Some(2);
        b.options.node_rank = Some(1);
        b.options.head_address = Some("192.168.37.107".into());
        b.options.host_address = Some("192.168.37.108".into());
        let value = serde_json::to_value(&b).unwrap();
        let back: ConvertRecipeRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(back).unwrap(), value);
        let preview = ConvertedRecipe {
            source_format: "compose".into(),
            services: vec!["svc".into()],
            model_spec: json!({"command":["--flag"]}),
            required_inputs: vec!["Set user".into()],
            warnings: vec!["warning".into()],
        };
        let value = serde_json::to_value(&preview).unwrap();
        let back: ConvertedRecipe = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(back).unwrap(), value);
    }
}

#[cfg(test)]
mod preparation_import_tests {
    use super::*;
    #[test]
    fn public_page_import_preserves_compose_and_pins_requirements() {
        let page = r#"<div id="pane-compose"><pre><code>name: example
services:
  model:
    image: example@sha256:abc
    command: [&quot;--profile&quot;, &quot;example&quot;]
</code></pre></div><div id="pane-vllm"><p>Read only command.</p><pre><code>python -m vllm serve owner/model --revision aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --speculative-config &#39;{&quot;model&quot;:&quot;owner/draft&quot;,&quot;revision&quot;:&quot;bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb&quot;}&#39;</code></pre></div>"#;
        let imported = imported_lil_page(
            "https://docker.local-inference-lab.ai/?build=pinned&model=example",
            page,
        )
        .unwrap();
        assert!(
            imported
                .source_content
                .contains("command: [\"--profile\", \"example\"]")
        );
        assert_eq!(imported.checkpoints.len(), 2);
        assert_eq!(imported.checkpoints[0].repository, "owner/model");
        assert_eq!(imported.checkpoints[1].repository, "owner/draft");
        let serialized = serde_json::to_value(&imported).unwrap();
        let back: ImportedRecipeSource = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(serde_json::to_value(back).unwrap(), serialized);
    }
    #[test]
    fn no_guessed_revisions_or_arbitrary_page_scripts() {
        assert!(command_checkpoints("vllm serve owner/model --revision main").is_empty());
        assert!(
            imported_lil_page(
                "https://docker.local-inference-lab.ai/?build=a&model=b",
                "<script>fetch('/private')</script>"
            )
            .is_err()
        );
        assert_eq!(decode_code("&amp;&lt;&gt;&#39;&#34;").unwrap(), "&<>'\"");
    }
}

#[cfg(test)]
mod live_lil_fixture_tests {
    use super::*;
    #[test]
    fn deepseek_page_imports_exact_compose_and_csf_revision() {
        let source=imported_lil_page("https://docker.local-inference-lab.ai/?build=karmic-kraken-beta-08edd314f7ee880c5cc29eb4692afc2aa7a2c51736c188335b655cdbf5ed6eea&model=ds41-flash&cache-l1-gib=512&cache=ram",include_str!("../tests/fixtures/lil-ds41-preparation.html")).unwrap();
        assert_eq!(source.checkpoints.len(), 1);
        assert_eq!(
            source.checkpoints[0].repository,
            "local-inference-lab/DeepSeek-V4.1-Flash-lossless-CSF"
        );
        assert_eq!(
            source.checkpoints[0].revision,
            "9c60bcbf1670e06f154b5d61080e50c9b3b7b4a3"
        );
        assert!(
            source.source_content.contains(
                "sha256:f5f9efe1b10ec7b853a495e66b7dbf15ff54f62741e444dea0b44e16736a10a9"
            )
        );
        assert!(source.source_content.contains("--no-cache-l2-enabled"));
    }
}
