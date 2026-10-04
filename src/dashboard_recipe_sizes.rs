//! Named sizes resolve once into immutable executable revisions, keeping evidence
//! specific to the actual launch settings instead of the parent recipe family.
use crate::dashboard_management::{CreateRecipeRevision, ManagementState, RecipeRevision};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Size {
    #[serde(default)]
    description: String,
    #[serde(default)]
    overrides: BTreeMap<String, Value>,
    #[serde(default)]
    vllm: BTreeMap<String, Value>,
}
fn apply(model: &mut Value, size: &Size) -> Result<(), &'static str> {
    let object = model
        .as_object_mut()
        .ok_or("size needs an executable recipe")?;
    for (key, value) in &size.overrides {
        if matches!(
            key.as_str(),
            "sizes"
                | "distributed"
                | "source_format"
                | "source_content"
                | "metadata"
                | "id"
                | "placement"
        ) {
            return Err("size cannot override recipe identity or catalog fields");
        }
        if key == "environment" {
            let env = value
                .as_object()
                .ok_or("size environment must be an object")?;
            let target = object
                .entry(key.clone())
                .or_insert(json!({}))
                .as_object_mut()
                .ok_or("invalid environment")?;
            target.extend(env.clone());
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    let kv_size = regex::Regex::new(r"^[1-9][0-9]*(?:[kKmMgG]|KiB|MiB|GiB)?$").unwrap();
    for (parameter, value) in &size.vllm {
        let (flag, env) = match parameter.as_str() {
            "gpu_memory_utilization" => ("gpu-memory-utilization", "GPU_MEMORY_UTILIZATION"),
            "kv_cache_memory_bytes" => ("kv-cache-memory-bytes", "KV_CACHE_MEMORY_BYTES"),
            "max_model_len" => ("max-model-len", "MAX_MODEL_LEN"),
            "max_num_seqs" => ("max-num-seqs", "MAX_NUM_SEQS"),
            "max_num_batched_tokens" => ("max-num-batched-tokens", "MAX_BATCHED_TOKENS"),
            _ => return Err("unsupported size vLLM parameter"),
        };
        let literal = if parameter == "gpu_memory_utilization" {
            value
                .as_f64()
                .filter(|n| n.is_finite() && *n > 0.0 && *n <= 1.0)
                .ok_or("GPU utilization must be in (0,1]")?
                .to_string()
        } else if parameter == "kv_cache_memory_bytes" {
            let literal = value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_u64().filter(|n| *n > 0).map(|n| n.to_string()))
                .ok_or("KV cache must be a positive size")?;
            if !kv_size.is_match(&literal) {
                return Err("invalid KV cache size");
            }
            literal
        } else {
            value
                .as_u64()
                .filter(|n| *n > 0 && *n <= 10_000_000)
                .ok_or("size token/sequence limits must be positive integers")?
                .to_string()
        };
        let mut changed = false;
        if let Some(environment) = object.get_mut("environment").and_then(Value::as_object_mut)
            && environment.contains_key(env)
        {
            environment.insert(env.into(), json!(literal));
            changed = true;
        }
        if let Some(command) = object.get_mut("command").and_then(Value::as_array_mut) {
            let exact = format!("--{flag}");
            let expression = regex::Regex::new(&format!(
                r"(^|[\s])(--{}(?:[ \t]+|=))([0-9][A-Za-z0-9.+-]*)([\s]|$)",
                regex::escape(flag)
            ))
            .unwrap();
            let mut i = 0;
            while i < command.len() {
                let arg = command[i]
                    .as_str()
                    .ok_or("command must contain strings")?
                    .to_owned();
                if arg == exact {
                    if i + 1 >= command.len() {
                        return Err("vLLM flag has no value");
                    }
                    command[i + 1] = json!(literal);
                    changed = true;
                    i += 2;
                    continue;
                }
                if arg.starts_with(&format!("{exact}=")) {
                    command[i] = json!(format!("{exact}={literal}"));
                    changed = true;
                } else if expression.is_match(&arg) {
                    command[i] = json!(
                        expression
                            .replace_all(&arg, |c: &regex::Captures<'_>| format!(
                                "{}{}{}{}",
                                &c[1], &c[2], literal, &c[4]
                            ))
                            .to_string()
                    );
                    changed = true;
                }
                i += 1;
            }
        }
        if !changed {
            return Err(
                "size vLLM override must match an existing literal flag or environment setting",
            );
        }
        if parameter == "gpu_memory_utilization"
            && size
                .overrides
                .get("gpu_memory")
                .and_then(|v| v.get("fraction"))
                .is_none()
            && let Some(memory) = object.get_mut("gpu_memory").and_then(Value::as_object_mut)
        {
            memory.insert("fraction".into(), value.clone());
        }
    }
    Ok(())
}
pub(crate) fn resolve(
    spec: &Value,
    name: &str,
    source_digest: &str,
) -> Result<Value, &'static str> {
    if !crate::dashboard_management::valid_id(name) {
        return Err("invalid size name");
    }
    let definition = spec
        .get("sizes")
        .and_then(Value::as_object)
        .and_then(|s| s.get(name))
        .ok_or("named recipe size does not exist")?;
    let size: Size =
        serde_json::from_value(definition.clone()).map_err(|_| "invalid size definition")?;
    if size.description.len() > 256 {
        return Err("size description exceeds 256 bytes");
    }
    let mut result = spec.clone();
    result
        .as_object_mut()
        .ok_or("invalid recipe")?
        .remove("sizes");
    if let Some(ranks) = result.get_mut("distributed").and_then(Value::as_array_mut) {
        for rank in ranks {
            apply(rank, &size)?;
        }
    } else {
        apply(&mut result, &size)?;
    }
    let metadata = result
        .as_object_mut()
        .unwrap()
        .entry("metadata")
        .or_insert(json!({}))
        .as_object_mut()
        .ok_or("invalid recipe metadata")?;
    metadata.insert("size_name".into(), json!(name));
    metadata.insert("size_source_digest".into(), json!(source_digest));
    Ok(result)
}
pub(crate) fn validate(spec: &Value) -> Result<(), &'static str> {
    let Some(sizes) = spec.get("sizes") else {
        return Ok(());
    };
    let sizes = sizes
        .as_object()
        .filter(|s| s.len() <= 16)
        .ok_or("sizes must be an object with at most 16 named sizes")?;
    for name in sizes.keys() {
        crate::dashboard_management::validate_recipe(&CreateRecipeRevision {
            description: String::new(),
            model_spec: resolve(spec, name, &"0".repeat(64))?,
        })?;
    }
    Ok(())
}
pub(crate) fn pin(
    state: &mut ManagementState,
    recipe_id: &str,
    digest: &str,
    size: Option<&str>,
) -> Result<RecipeRevision, &'static str> {
    let revisions = state
        .recipes
        .get_mut(recipe_id)
        .ok_or("recipe does not exist")?;
    let source = revisions
        .iter()
        .find(|r| r.digest == digest)
        .cloned()
        .ok_or("recipe revision does not exist")?;
    let Some(size) = size else {
        return Ok(source);
    };
    let spec = resolve(&source.model_spec, size, digest)?;
    let mut prefix = source.description.clone();
    while prefix.len() > 180 {
        prefix.pop();
    }
    let description = format!("{prefix} · {size}");
    let digest = crate::dashboard_management::validate_recipe(&CreateRecipeRevision {
        description: description.clone(),
        model_spec: spec.clone(),
    })?;
    if let Some(existing) = revisions.iter().find(|r| r.digest == digest) {
        return Ok(existing.clone());
    }
    if revisions.len() >= 32 {
        return Err("revision limit reached");
    }
    let revision = RecipeRevision {
        digest,
        revision: revisions.len() as u64 + 1,
        description,
        model_spec: spec,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    revisions.push(revision.clone());
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe() -> Value {
        json!({"image":format!("sha256:{}","a".repeat(64)),"user":"1000:1000","gpus":"device=0","memory_limit":"100g","command":["bash","-c","exec vllm serve model --gpu-memory-utilization 0.9 \\\n --kv-cache-memory-bytes 8G --max-num-seqs 64"],"sizes":{"compact":{"overrides":{"memory_limit":"40g","gpu_memory":{"mode":"shared","mib_per_device":32768}},"vllm":{"gpu_memory_utilization":0.3,"kv_cache_memory_bytes":"4G","max_num_seqs":16}}}})
    }
    #[test]
    fn size_pinning_is_immutable_validated_and_deduplicated() {
        let spec = recipe();
        let digest = crate::dashboard_management::validate_recipe(&CreateRecipeRevision {
            description: "Model".into(),
            model_spec: spec.clone(),
        })
        .unwrap();
        let mut state = ManagementState::default();
        state.recipes.insert(
            "model".into(),
            vec![RecipeRevision {
                digest: digest.clone(),
                revision: 1,
                description: "Model".into(),
                model_spec: spec.clone(),
                created_at_ms: 1,
            }],
        );
        let resolved = pin(&mut state, "model", &digest, Some("compact")).unwrap();
        assert_ne!(resolved.digest, digest);
        assert_eq!(resolved.model_spec["memory_limit"], "40g");
        assert_eq!(resolved.model_spec["gpu_memory"]["fraction"], 0.3);
        let command = resolved.model_spec["command"][2].as_str().unwrap();
        assert!(command.contains("--kv-cache-memory-bytes 4G"));
        assert!(command.contains("--max-num-seqs 16"));
        assert!(resolved.model_spec.get("sizes").is_none());
        assert_eq!(state.recipes["model"][0].model_spec, spec);
        assert_eq!(
            pin(&mut state, "model", &digest, Some("compact"))
                .unwrap()
                .digest,
            resolved.digest
        );
        assert_eq!(state.recipes["model"].len(), 2);
        let roundtrip: ManagementState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            roundtrip.recipes["model"][1].model_spec,
            resolved.model_spec
        );
    }
    #[test]
    fn sizes_reject_unmatched_flags_credentials_and_bad_values() {
        let mut spec = recipe();
        spec["sizes"]["compact"]["vllm"]["gpu_memory_utilization"] = json!(1.5);
        assert!(validate(&spec).is_err());
        spec = recipe();
        spec["sizes"]["compact"]["vllm"]["kv_cache_memory_bytes"] = json!("4G; cat /secret");
        assert!(validate(&spec).is_err());
        spec = recipe();
        spec["sizes"]["compact"]["vllm"]["max_model_len"] = json!(1024);
        assert!(validate(&spec).is_err());
        spec = recipe();
        spec["sizes"]["compact"]["overrides"]["environment"] = json!({"HF_TOKEN":"secret"});
        assert!(validate(&spec).is_err());
        assert!(resolve(&spec, "missing", "digest").is_err());
    }
    #[test]
    fn argv_and_environment_overrides_preserve_unrelated_settings() {
        let mut spec = recipe();
        spec["command"] = json!([
            "vllm",
            "serve",
            "model",
            "--gpu-memory-utilization=0.9",
            "--kv-cache-memory-bytes",
            "8G",
            "--max-num-seqs",
            "64"
        ]);
        spec["environment"] = json!({"MAX_NUM_SEQS":"64","OTHER":"preserved"});
        let resolved = resolve(&spec, "compact", &"a".repeat(64)).unwrap();
        assert_eq!(resolved["command"][3], "--gpu-memory-utilization=0.3");
        assert_eq!(resolved["command"][5], "4G");
        assert_eq!(resolved["environment"]["MAX_NUM_SEQS"], "16");
        assert_eq!(resolved["environment"]["OTHER"], "preserved");
    }
    #[test]
    fn calibrated_fixed_cache_budget_can_override_utilization_fraction() {
        let mut spec = recipe();
        spec["sizes"]["compact"]["overrides"]["gpu_memory"]["fraction"] = json!(0.0);
        let resolved = resolve(&spec, "compact", &"a".repeat(64)).unwrap();
        assert_eq!(resolved["gpu_memory"]["fraction"], 0.0);
        assert!(
            resolved["command"][2]
                .as_str()
                .unwrap()
                .contains("--gpu-memory-utilization 0.3")
        );
    }
}
