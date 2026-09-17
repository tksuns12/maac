//! Generic MaaC/1 Locked Render verification (§22 / Generic interchange v1).
//!
//! This module verifies a supplied lock against an already-resolved host
//! context. It deliberately does not discover dependencies or interpret unknown
//! processor/adapter contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::production_identity::canonical_json_bytes;

const MAX_LOCK_BYTES: usize = 16 * 1024 * 1024;
const MAX_VALUES: usize = 250_000;
const MAX_DEPTH: usize = 96;
const MAX_RATIONAL_DECIMAL_DIGITS: usize = 1234;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockError {
    pub code: &'static str,
    pub message: String,
}
impl LockError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for LockError {}
type Result<T> = std::result::Result<T, LockError>;

#[derive(Clone, Debug)]
pub struct GenericLock {
    value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct DependencyIdentity {
    pub owner: Option<Vec<String>>,
    pub role: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExpectedAsset {
    pub kind: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ExpectedProcessor {
    pub processor_type: String,
    pub implementation_hash: Option<String>,
    pub descriptor_hash: Option<String>,
    pub adapter_id: Option<String>,
    pub state_hash: Option<String>,
    pub config: Value,
    pub latency_frames: u64,
    pub determinism: String,
}
#[derive(Clone, Debug)]
pub struct ExpectedEngine {
    pub implementation_id: String,
    pub build_id: String,
    pub platform_id: String,
    pub architecture_id: String,
    pub numerical_mode_id: String,
    pub sample_rate: u64,
    /// Exact reset-to-render block schedule required by the understood host
    /// contract. `None` is valid only when `block_independent` is proven.
    pub block_schedule: Option<Vec<u64>>,
    pub block_independent: bool,
}
#[derive(Clone, Debug)]
pub struct ExpectedOutput {
    pub port: Value,
    pub score: [Value; 2],
    pub tail: Value,
    pub render_frames: u64,
    pub crop: (u64, u64),
    pub channel_order: Vec<u64>,
}
#[derive(Clone, Debug)]
pub struct LockVerificationContext {
    pub execution_preimage: Value,
    pub assets: BTreeMap<Vec<String>, ExpectedAsset>,
    pub dependencies: BTreeMap<DependencyIdentity, Vec<u8>>,
    pub processors: BTreeMap<Vec<String>, ExpectedProcessor>,
    pub engine: ExpectedEngine,
    pub output: ExpectedOutput,
    pub pcm: Option<Vec<u8>>,
    pub file: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedLock {
    pub execution_hash: String,
    pub render_key: String,
    pub crop: (u64, u64),
    pub channels: usize,
    pub pcm_verified: bool,
    pub file_verified: bool,
}

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded unique-key integer JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Unique, E> {
                Err(E::custom("JSON floats are forbidden"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut x = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    if x.len() >= MAX_VALUES {
                        return Err(de::Error::custom("value limit"));
                    }
                    x.push(v);
                }
                Ok(Unique(Value::Array(x)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut m = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if m.contains_key(&k) {
                        return Err(de::Error::custom(format!("duplicate JSON key `{k}`")));
                    }
                    let Unique(v) = a.next_value()?;
                    m.insert(k, v);
                }
                Ok(Unique(Value::Object(m)))
            }
        }
        deserializer.deserialize_any(V)
    }
}

impl GenericLock {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_LOCK_BYTES {
            return Err(LockError::new(
                "E_RESOURCE_LIMIT",
                "lock byte limit exceeded",
            ));
        }
        preflight_depth(bytes)?;
        let mut d = serde_json::Deserializer::from_slice(bytes);
        let Unique(value) =
            Unique::deserialize(&mut d).map_err(|e| LockError::new("E_SCHEMA", e.to_string()))?;
        d.end()
            .map_err(|e| LockError::new("E_SCHEMA", e.to_string()))?;
        preflight_value(&value)?;
        let lock = Self { value };
        lock.validate()?;
        Ok(lock)
    }
    pub fn from_value(value: Value) -> Result<Self> {
        preflight_value(&value)?;
        let bytes =
            canonical_json_bytes(&value).map_err(|e| LockError::new("E_SCHEMA", e.to_string()))?;
        if bytes.len() > MAX_LOCK_BYTES {
            return Err(LockError::new(
                "E_RESOURCE_LIMIT",
                "lock byte limit exceeded",
            ));
        }
        let lock = Self { value };
        lock.validate()?;
        Ok(lock)
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn render_key(&self) -> &str {
        self.value["render_key"].as_str().expect("validated")
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canon(&self.value)
    }
    pub fn whole_lock_digest(&self) -> Result<String> {
        Ok(sha_uri(&self.canonical_bytes()?))
    }
    pub fn render_input(&self) -> Result<Value> {
        Ok(derive_render_input(obj(&self.value, "lock")?))
    }
    pub fn render_input_bytes(&self) -> Result<Vec<u8>> {
        canon(&self.render_input()?)
    }
    pub fn validate(&self) -> Result<()> {
        validate_lock(&self.value)
    }
    pub fn verify(&self, context: &LockVerificationContext) -> Result<VerifiedLock> {
        self.validate()?;
        let v = obj(&self.value, "lock")?;
        let execution = sha_uri(
            &canonical_json_bytes(&context.execution_preimage)
                .map_err(|e| LockError::new("E_DIGEST", e.to_string()))?,
        );
        if v["execution_hash"] != execution {
            return Err(LockError::new(
                "E_DIGEST",
                "execution_hash differs from resolved N(A)",
            ));
        }
        verify_assets(v, context)?;
        verify_dependencies(v, context)?;
        verify_processors(v, context)?;
        verify_engine(v, context)?;
        verify_output(v, context)?;
        let evidence = v.get("evidence").expect("validated");
        let mut pcm_ok = false;
        let mut file_ok = false;
        if !evidence.is_null() {
            let e = obj(evidence, "evidence")?;
            let pcm = context.pcm.as_ref().ok_or_else(|| {
                LockError::new(
                    "E_EVIDENCE",
                    "lock contains PCM evidence but caller supplied no PCM",
                )
            })?;
            if e["pcm_sha256"] != sha_uri(pcm)
                || u(&e["pcm_bytes"], "pcm_bytes")? != pcm.len() as u64
            {
                return Err(LockError::new(
                    "E_EVIDENCE",
                    "PCM evidence hash/length mismatch",
                ));
            }
            let out = obj(&v["output"], "output")?;
            let crop = arr(&out["crop"], "crop")?;
            let frames = u(&crop[1], "crop end")?
                .checked_sub(u(&crop[0], "crop start")?)
                .ok_or_else(|| LockError::new("E_TIMING", "evidence crop is reversed"))?;
            let channels = u64::try_from(arr(&out["channel_order"], "channel_order")?.len())
                .map_err(|_| LockError::new("E_RESOURCE_LIMIT", "channel count overflows u64"))?;
            let expected_bytes = frames
                .checked_mul(channels)
                .and_then(|value| value.checked_mul(4))
                .ok_or_else(|| LockError::new("E_RESOURCE_LIMIT", "PCM byte length overflows"))?;
            if u64::try_from(pcm.len()).ok() != Some(expected_bytes) {
                return Err(LockError::new(
                    "E_EVIDENCE",
                    "PCM byte length differs from crop × channels × 4",
                ));
            }
            for chunk in pcm.chunks_exact(4) {
                if !f32::from_le_bytes(chunk.try_into().unwrap()).is_finite() {
                    return Err(LockError::new(
                        "E_EVIDENCE",
                        "PCM contains nonfinite binary32",
                    ));
                }
            }
            pcm_ok = true;
            if !e["file"].is_null() {
                let f = obj(&e["file"], "evidence.file")?;
                let bytes = context.file.as_ref().ok_or_else(|| {
                    LockError::new(
                        "E_EVIDENCE",
                        "file evidence present but caller supplied no file",
                    )
                })?;
                if f["sha256"] != sha_uri(bytes)
                    || u(&f["bytes"], "file.bytes")? != bytes.len() as u64
                    || bytes != pcm
                {
                    return Err(LockError::new(
                        "E_EVIDENCE",
                        "file evidence hash/length mismatch",
                    ));
                }
                file_ok = true;
            }
        }
        let out = obj(&v["output"], "output")?;
        let crop = arr(&out["crop"], "crop")?;
        Ok(VerifiedLock {
            execution_hash: execution,
            render_key: self.render_key().into(),
            crop: (u(&crop[0], "crop")?, u(&crop[1], "crop")?),
            channels: arr(&out["channel_order"], "channel_order")?.len(),
            pcm_verified: pcm_ok,
            file_verified: file_ok,
        })
    }
}

fn preflight_value(value: &Value) -> Result<()> {
    let mut stack = vec![(value, 1usize)];
    let mut count = 0usize;
    while let Some((value, depth)) = stack.pop() {
        count = count
            .checked_add(1)
            .ok_or_else(|| LockError::new("E_RESOURCE_LIMIT", "value count overflow"))?;
        if count > MAX_VALUES {
            return Err(LockError::new(
                "E_RESOURCE_LIMIT",
                "lock value limit exceeded",
            ));
        }
        if depth > MAX_DEPTH {
            return Err(LockError::new(
                "E_RESOURCE_LIMIT",
                "JSON nesting limit exceeded",
            ));
        }
        match value {
            Value::Array(items) => {
                stack.extend(items.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(fields) => {
                stack.extend(fields.values().map(|item| (item, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn preflight_depth(bytes: &[u8]) -> Result<()> {
    let (mut depth, mut quoted, mut esc) = (0usize, false, false);
    for b in bytes {
        if quoted {
            if esc {
                esc = false
            } else if *b == b'\\' {
                esc = true
            } else if *b == b'"' {
                quoted = false
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err(LockError::new(
                            "E_RESOURCE_LIMIT",
                            "JSON nesting limit exceeded",
                        ));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}
fn obj<'a>(v: &'a Value, c: &str) -> Result<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| LockError::new("E_SCHEMA", format!("{c}: expected object")))
}
fn arr<'a>(v: &'a Value, c: &str) -> Result<&'a Vec<Value>> {
    v.as_array()
        .ok_or_else(|| LockError::new("E_SCHEMA", format!("{c}: expected array")))
}
fn s<'a>(v: &'a Value, c: &str) -> Result<&'a str> {
    v.as_str()
        .filter(|x| !x.is_empty())
        .ok_or_else(|| LockError::new("E_SCHEMA", format!("{c}: expected nonempty string")))
}
fn exact(m: &Map<String, Value>, keys: &[&str], c: &str) -> Result<()> {
    let a = m.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let b = keys.iter().copied().collect::<BTreeSet<_>>();
    if a != b {
        return Err(LockError::new("E_SCHEMA", format!("{c}: wrong field set")));
    }
    Ok(())
}
fn digest<'a>(v: &'a Value, c: &str) -> Result<&'a str> {
    let x = s(v, c)?;
    if x.len() != 71
        || !x.starts_with("sha256:")
        || !x[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(LockError::new("E_SCHEMA", format!("{c}: invalid SHA-256")));
    }
    Ok(x)
}
fn u(v: &Value, c: &str) -> Result<u64> {
    let x = s(v, c)?;
    if x != "0" && (x.starts_with('0') || !x.bytes().all(|b| b.is_ascii_digit())) {
        return Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: noncanonical unsigned integer"),
        ));
    }
    x.parse()
        .map_err(|_| LockError::new("E_SCHEMA", format!("{c}: unsigned integer overflow")))
}
fn positive(v: &Value, c: &str) -> Result<u64> {
    let x = u(v, c)?;
    if x == 0 {
        Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: expected positive integer"),
        ))
    } else {
        Ok(x)
    }
}
fn identifier<'a>(v: &'a Value, c: &str) -> Result<&'a str> {
    let q = s(v, c)?;
    let valid = q.len() <= 128
        && q.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic() || byte == b'_'
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_'
            }
        });
    if !valid {
        return Err(LockError::new("E_SCHEMA", format!("{c}: invalid MaaC ID")));
    }
    Ok(q)
}
fn path(v: &Value, c: &str) -> Result<Vec<String>> {
    let a = arr(v, c)?;
    if a.is_empty() {
        return Err(LockError::new("E_SCHEMA", format!("{c}: empty path")));
    }
    a.iter()
        .map(|x| identifier(x, c).map(str::to_owned))
        .collect()
}
fn sha_uri(raw: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(raw))
}
fn canon(v: &Value) -> Result<Vec<u8>> {
    canonical_json_bytes(v).map_err(|e| LockError::new("E_SCHEMA", e.to_string()))
}
fn dep_id(v: &Value) -> Result<DependencyIdentity> {
    let m = obj(v, "dependency")?;
    Ok(DependencyIdentity {
        owner: if m["owner"].is_null() {
            None
        } else {
            Some(path(&m["owner"], "owner")?)
        },
        role: arr(&m["role"], "role")?
            .iter()
            .map(|x| s(x, "role").map(str::to_owned))
            .collect::<Result<_>>()?,
    })
}

fn typed(v: &Value, c: &str) -> Result<()> {
    let m = obj(v, c)?;
    let t = s(m.get("t").unwrap_or(&Value::Null), c)?;
    match t {
        "number" => {
            exact(m, &["t", "n", "d"], c)?;
            rat(m, c)?
        }
        "quantity" => {
            exact(m, &["t", "n", "d", "u"], c)?;
            rat(m, c)?;
            if !matches!(
                s(&m["u"], c)?,
                "q" | "s" | "ms" | "frame" | "Hz" | "kHz" | "bpm" | "ct" | "dB"
            ) {
                return Err(LockError::new(
                    "E_SCHEMA",
                    format!("{c}: unknown quantity unit"),
                ));
            }
        }
        "string" | "symbol" => {
            exact(m, &["t", "v"], c)?;
            if !m["v"].is_string() {
                return Err(LockError::new("E_SCHEMA", format!("{c}: typed string")));
            }
        }
        "boolean" => {
            exact(m, &["t", "v"], c)?;
            if !m["v"].is_boolean() {
                return Err(LockError::new("E_SCHEMA", format!("{c}: typed boolean")));
            }
        }
        "ref" => {
            exact(m, &["t", "path", "port"], c)?;
            path(&m["path"], c)?;
            if !m["port"].is_null() {
                identifier(&m["port"], c)?;
            }
        }
        "record" => {
            exact(m, &["t", "fields"], c)?;
            for (name, x) in obj(&m["fields"], c)? {
                let key = Value::String(name.clone());
                identifier(&key, c)?;
                typed(x, c)?;
            }
        }
        "list" | "tuple" => {
            exact(m, &["t", "items"], c)?;
            let items = arr(&m["items"], c)?;
            if t == "tuple" && items.len() < 2 {
                return Err(LockError::new(
                    "E_SCHEMA",
                    format!("{c}: tuple must contain at least two items"),
                ));
            }
            for x in items {
                typed(x, c)?;
            }
        }
        "call" => {
            exact(m, &["t", "fn", "args"], c)?;
            identifier(&m["fn"], c)?;
            for x in arr(&m["args"], c)? {
                typed(x, c)?;
            }
        }
        _ => {
            return Err(LockError::new(
                "E_SCHEMA",
                format!("{c}: unknown typed tag"),
            ))
        }
    }
    Ok(())
}
fn rat(m: &Map<String, Value>, c: &str) -> Result<()> {
    let n = s(&m["n"], c)?;
    let d = s(&m["d"], c)?;
    let canonical_integer = |text: &str, signed: bool| {
        let digits = if signed {
            text.strip_prefix('-').unwrap_or(text)
        } else {
            text
        };
        !(digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || text.starts_with('+')
            || (digits.len() > 1 && digits.starts_with('0')))
            && (!text.starts_with('-') || (signed && digits != "0"))
    };
    if !canonical_integer(n, true) || !canonical_integer(d, false) {
        return Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: noncanonical rational integer spelling"),
        ));
    }
    let numerator_digits = n.strip_prefix('-').unwrap_or(n).len();
    if numerator_digits > MAX_RATIONAL_DECIMAL_DIGITS || d.len() > MAX_RATIONAL_DECIMAL_DIGITS {
        return Err(LockError::new(
            "E_RESOURCE_LIMIT",
            format!("{c}: rational exceeds the bit allowance"),
        ));
    }
    let ni: num_bigint::BigInt = n
        .parse()
        .map_err(|_| LockError::new("E_SCHEMA", format!("{c}: bad rational")))?;
    let di: num_bigint::BigInt = d
        .parse()
        .map_err(|_| LockError::new("E_SCHEMA", format!("{c}: bad rational")))?;
    if ni.magnitude().bits() > crate::MAX_RATIONAL_BITS
        || di.magnitude().bits() > crate::MAX_RATIONAL_BITS
    {
        return Err(LockError::new(
            "E_RESOURCE_LIMIT",
            format!("{c}: rational exceeds the bit allowance"),
        ));
    }
    if di <= 0.into() || num_integer::Integer::gcd(&ni, &di) != 1.into() {
        return Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: rational not reduced"),
        ));
    }
    Ok(())
}

fn rational_value(v: &Value, c: &str) -> Result<num_rational::BigRational> {
    let m = obj(v, c)?;
    rat(m, c)?;
    let n = m["n"]
        .as_str()
        .expect("rat validated numerator")
        .parse::<num_bigint::BigInt>()
        .map_err(|_| LockError::new("E_SCHEMA", format!("{c}: bad rational")))?;
    let d = m["d"]
        .as_str()
        .expect("rat validated denominator")
        .parse::<num_bigint::BigInt>()
        .map_err(|_| LockError::new("E_SCHEMA", format!("{c}: bad rational")))?;
    Ok(num_rational::BigRational::new(n, d))
}

fn validate_config(v: &Value, c: &str) -> Result<()> {
    let m = obj(v, c)?;
    exact(
        m,
        &[
            "format",
            "version",
            "processor_type",
            "descriptor_hash",
            "config",
        ],
        c,
    )?;
    if m["format"] != "maac.generic-config" || m["version"] != 1 {
        return Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: bad format/version"),
        ));
    }
    s(&m["processor_type"], c)?;
    if !m["descriptor_hash"].is_null() {
        digest(&m["descriptor_hash"], c)?;
    }
    typed(&m["config"], c)?;
    if m["config"]["t"] != "record" {
        return Err(LockError::new(
            "E_SCHEMA",
            format!("{c}: config not record"),
        ));
    }
    Ok(())
}
fn validate_lock(v: &Value) -> Result<()> {
    let m = obj(v, "lock")?;
    exact(
        m,
        &[
            "format",
            "version",
            "execution_hash",
            "assets",
            "dependencies",
            "processors",
            "engine",
            "output",
            "render_key",
            "evidence",
        ],
        "lock",
    )?;
    if m["format"] != "maac.generic-lock" || m["version"] != 1 {
        return Err(LockError::new(
            "E_SCHEMA",
            "unsupported lock format/version",
        ));
    }
    digest(&m["execution_hash"], "execution_hash")?;
    digest(&m["render_key"], "render_key")?;
    let assets = arr(&m["assets"], "assets")?;
    let mut ap = Vec::new();
    for (a_i, a) in assets.iter().enumerate() {
        let q = obj(a, "asset")?;
        exact(q, &["source", "kind", "sha256", "bytes"], "asset")?;
        let id = path(&q["source"], "asset.source")?;
        if !matches!(
            s(&q["kind"], "asset.kind")?,
            "audio" | "blob" | "descriptor" | "module"
        ) {
            return Err(LockError::new("E_SCHEMA", "bad asset kind"));
        }
        digest(&q["sha256"], "asset.sha256")?;
        u(&q["bytes"], "asset.bytes")?;
        ap.push((canon(&q["source"])?, id, a_i));
    }
    if ap
        .iter()
        .map(|item| item.1.clone())
        .collect::<BTreeSet<_>>()
        .len()
        != ap.len()
    {
        return Err(LockError::new(
            "E_CLOSURE",
            "duplicate asset source identity",
        ));
    }
    if ap.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(LockError::new("E_ORDER", "assets are not sorted"));
    }
    let deps = arr(&m["dependencies"], "dependencies")?;
    let mut di = Vec::new();
    let mut engines = 0;
    for d in deps {
        let q = obj(d, "dependency")?;
        exact(q, &["owner", "role", "sha256", "bytes"], "dependency")?;
        let id = dep_id(d)?;
        let role = &id.role;
        let valid = match (&id.owner, role.as_slice()) {
            (None, [a, b]) if a == "engine" && b == "implementation" => {
                engines += 1;
                true
            }
            (Some(_), [a, b])
                if a == "processor"
                    && matches!(
                        b.as_str(),
                        "implementation" | "descriptor" | "adapter" | "state"
                    ) =>
            {
                true
            }
            (_, [a, _, _]) if a == "imported-source" || a == "auxiliary" => true,
            _ => false,
        };
        if !valid {
            return Err(LockError::new("E_SCHEMA", "invalid dependency role"));
        }
        digest(&q["sha256"], "dependency.sha256")?;
        u(&q["bytes"], "dependency.bytes")?;
        di.push(canon(&json!({"owner":q["owner"],"role":q["role"]}))?);
    }
    if engines != 1 {
        return Err(LockError::new(
            "E_SCHEMA",
            "exactly one engine implementation dependency required",
        ));
    }
    if di.iter().cloned().collect::<BTreeSet<_>>().len() != di.len() {
        return Err(LockError::new("E_CLOSURE", "duplicate dependency identity"));
    }
    if di.windows(2).any(|w| w[0] >= w[1]) {
        return Err(LockError::new("E_ORDER", "dependencies are not sorted"));
    }
    let ps = arr(&m["processors"], "processors")?;
    let mut pi = Vec::new();
    let mut processor_nodes = BTreeSet::new();
    for p in ps {
        validate_processor(p)?;
        processor_nodes.insert(path(&p["node"], "processor.node")?);
        pi.push(canon(&p["node"])?);
    }
    for dependency in deps {
        let id = dep_id(dependency)?;
        if id.role.first().map(String::as_str) == Some("processor")
            && id
                .owner
                .as_ref()
                .is_none_or(|owner| !processor_nodes.contains(owner))
        {
            return Err(LockError::new(
                "E_CLOSURE",
                "processor-owned dependency refers to no processor",
            ));
        }
    }
    if pi.iter().cloned().collect::<BTreeSet<_>>().len() != pi.len() {
        return Err(LockError::new(
            "E_CLOSURE",
            "duplicate processor node identity",
        ));
    }
    if pi.windows(2).any(|w| w[0] >= w[1]) {
        return Err(LockError::new("E_ORDER", "processors are not sorted"));
    }
    validate_engine(&m["engine"])?;
    validate_output(&m["output"])?;
    let render_frames = u(&m["output"]["render_frames"], "render_frames")?;
    if !m["engine"]["block_schedule"].is_null() {
        let total = arr(&m["engine"]["block_schedule"], "block_schedule")?
            .iter()
            .try_fold(0u64, |sum, block| {
                positive(block, "block").and_then(|n| {
                    sum.checked_add(n)
                        .ok_or_else(|| LockError::new("E_TIMING", "block schedule overflow"))
                })
            })?;
        if total != render_frames {
            return Err(LockError::new(
                "E_TIMING",
                "block schedule does not sum to render_frames",
            ));
        }
    }
    let order = arr(&m["output"]["channel_order"], "channel_order")?;
    let mut channels = order
        .iter()
        .map(|item| u(item, "channel"))
        .collect::<Result<Vec<_>>>()?;
    channels.sort_unstable();
    if channels != (0..order.len() as u64).collect::<Vec<_>>() {
        return Err(LockError::new(
            "E_SCHEMA",
            "channel_order is not a full permutation",
        ));
    }
    if !m["evidence"].is_null() {
        validate_evidence(&m["evidence"], &m["output"])?;
    }
    // cross pins and config digests
    for p in ps {
        let q = obj(p, "processor")?;
        let config_digest = sha_uri(&canon(&q["config"])?);
        if q["config_digest"] != config_digest {
            return Err(LockError::new("E_DIGEST", "stale config_digest"));
        }
        if q["config"]["processor_type"] != q["processor_type"]
            || q["config"]["descriptor_hash"] != q["descriptor_hash"]
        {
            return Err(LockError::new("E_DIGEST", "Config context mismatch"));
        }
        let node = path(&q["node"], "node")?;
        let owned = deps
            .iter()
            .filter_map(|d| {
                dep_id(d)
                    .ok()
                    .filter(|id| {
                        id.owner.as_ref() == Some(&node)
                            && id.role.first().map(String::as_str) == Some("processor")
                    })
                    .map(|id| (id.role[1].clone(), d))
            })
            .collect::<BTreeMap<_, _>>();
        if q["implementation_hash"].is_null() {
            let required = if q["state_hash"].is_null() {
                BTreeSet::new()
            } else {
                BTreeSet::from(["state"])
            };
            if owned.keys().map(String::as_str).collect::<BTreeSet<_>>() != required {
                return Err(LockError::new(
                    "E_CLOSURE",
                    "core processor slot closure mismatch",
                ));
            }
            if !q["state_hash"].is_null() && owned["state"]["sha256"] != q["state_hash"] {
                return Err(LockError::new(
                    "E_CLOSURE",
                    "core processor state cross-pin mismatch",
                ));
            }
        } else {
            let mut req = BTreeSet::from(["implementation", "descriptor", "adapter"]);
            if !q["state_hash"].is_null() {
                req.insert("state");
            }
            if owned.keys().map(String::as_str).collect::<BTreeSet<_>>() != req {
                return Err(LockError::new(
                    "E_CLOSURE",
                    "processor slot closure mismatch",
                ));
            }
            if owned["implementation"]["sha256"] != q["implementation_hash"]
                || owned["descriptor"]["sha256"] != q["descriptor_hash"]
            {
                return Err(LockError::new(
                    "E_CLOSURE",
                    "processor role cross-pin mismatch",
                ));
            }
            if !q["state_hash"].is_null() && owned["state"]["sha256"] != q["state_hash"] {
                return Err(LockError::new("E_CLOSURE", "state role cross-pin mismatch"));
            }
        }
    }
    let ri = derive_render_input(m);
    let key = sha_uri(&canon(&ri)?);
    if m["render_key"] != key {
        return Err(LockError::new("E_DIGEST", "stale render_key"));
    }
    Ok(())
}
fn validate_processor(v: &Value) -> Result<()> {
    let m = obj(v, "processor")?;
    exact(
        m,
        &[
            "node",
            "processor_type",
            "implementation_hash",
            "descriptor_hash",
            "adapter_id",
            "state_hash",
            "config",
            "config_digest",
            "latency_frames",
            "determinism",
        ],
        "processor",
    )?;
    path(&m["node"], "node")?;
    s(&m["processor_type"], "processor_type")?;
    let group = [
        &m["implementation_hash"],
        &m["descriptor_hash"],
        &m["adapter_id"],
    ];
    if !(group.iter().all(|x| x.is_null()) || group.iter().all(|x| !x.is_null())) {
        return Err(LockError::new(
            "E_SCHEMA",
            "processor identity group must be all null or all nonnull",
        ));
    }
    if !m["implementation_hash"].is_null() {
        digest(&m["implementation_hash"], "implementation_hash")?;
        digest(&m["descriptor_hash"], "descriptor_hash")?;
        s(&m["adapter_id"], "adapter_id")?;
    }
    if !m["state_hash"].is_null() {
        digest(&m["state_hash"], "state_hash")?;
    }
    validate_config(&m["config"], "processor.config")?;
    digest(&m["config_digest"], "config_digest")?;
    u(&m["latency_frames"], "latency_frames")?;
    if !matches!(
        s(&m["determinism"], "determinism")?,
        "declared_deterministic" | "nondeterministic"
    ) {
        return Err(LockError::new("E_SCHEMA", "bad determinism"));
    }
    Ok(())
}
fn validate_engine(v: &Value) -> Result<()> {
    let m = obj(v, "engine")?;
    exact(
        m,
        &[
            "implementation_id",
            "build_id",
            "platform_id",
            "architecture_id",
            "numerical_mode_id",
            "sample_rate",
            "block_schedule",
        ],
        "engine",
    )?;
    for k in [
        "implementation_id",
        "build_id",
        "platform_id",
        "architecture_id",
        "numerical_mode_id",
    ] {
        s(&m[k], k)?;
    }
    positive(&m["sample_rate"], "sample_rate")?;
    if !m["block_schedule"].is_null() {
        let a = arr(&m["block_schedule"], "block_schedule")?;
        if a.is_empty() {
            return Err(LockError::new("E_SCHEMA", "empty block schedule"));
        }
        for x in a {
            positive(x, "block")?;
        }
    }
    Ok(())
}
fn validate_output(v: &Value) -> Result<()> {
    let m = obj(v, "output")?;
    exact(
        m,
        &[
            "port",
            "score",
            "tail",
            "render_frames",
            "crop",
            "channel_order",
            "encoding_id",
            "clipping_id",
            "dither_id",
        ],
        "output",
    )?;
    typed(&m["port"], "port")?;
    if m["port"]["t"] != "ref" || m["port"]["port"].is_null() {
        return Err(LockError::new(
            "E_SCHEMA",
            "output port must be typed port reference",
        ));
    }
    let sc = arr(&m["score"], "score")?;
    if sc.len() != 2 {
        return Err(LockError::new("E_SCHEMA", "score must have two endpoints"));
    }
    for x in sc {
        typed(x, "score")?;
        if x["t"] != "quantity" || x["u"] != "q" {
            return Err(LockError::new("E_SCHEMA", "score endpoint must be q"));
        }
    }
    if rational_value(&sc[0], "score start")? >= rational_value(&sc[1], "score end")? {
        return Err(LockError::new(
            "E_TIMING",
            "score interval must satisfy start < end",
        ));
    }
    typed(&m["tail"], "tail")?;
    if m["tail"]["t"] != "quantity"
        || m["tail"]["u"] != "s"
        || m["tail"]["n"].as_str().unwrap().starts_with('-')
    {
        return Err(LockError::new(
            "E_SCHEMA",
            "tail must be nonnegative seconds",
        ));
    }
    let frames = u(&m["render_frames"], "render_frames")?;
    let crop = arr(&m["crop"], "crop")?;
    if crop.len() != 2 {
        return Err(LockError::new("E_SCHEMA", "crop must have two endpoints"));
    }
    let a = u(&crop[0], "crop")?;
    let b = u(&crop[1], "crop")?;
    if a > b || b > frames {
        return Err(LockError::new("E_TIMING", "crop outside render interval"));
    }
    let order = arr(&m["channel_order"], "channel_order")?;
    if order.is_empty() {
        return Err(LockError::new("E_SCHEMA", "empty channel_order"));
    }
    let mut seen = BTreeSet::new();
    for x in order {
        let q = u(x, "channel")?;
        if !seen.insert(q) {
            return Err(LockError::new("E_SCHEMA", "duplicate channel"));
        }
    }
    if m["encoding_id"] != "pcm_f32le_interleaved/1"
        || m["clipping_id"] != "none"
        || m["dither_id"] != "none"
    {
        return Err(LockError::new("E_SCHEMA", "unsupported output policy"));
    }
    Ok(())
}
fn validate_evidence(v: &Value, out: &Value) -> Result<()> {
    let m = obj(v, "evidence")?;
    exact(m, &["pcm_sha256", "pcm_bytes", "file"], "evidence")?;
    digest(&m["pcm_sha256"], "pcm_sha256")?;
    let n = u(&m["pcm_bytes"], "pcm_bytes")?;
    let o = obj(out, "output")?;
    let crop = arr(&o["crop"], "crop")?;
    let frames = u(&crop[1], "crop")?
        .checked_sub(u(&crop[0], "crop")?)
        .ok_or_else(|| LockError::new("E_TIMING", "evidence crop is reversed"))?;
    let channels = u64::try_from(arr(&o["channel_order"], "channel_order")?.len())
        .map_err(|_| LockError::new("E_RESOURCE_LIMIT", "channel count overflows u64"))?;
    let expected = frames
        .checked_mul(channels)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| LockError::new("E_RESOURCE_LIMIT", "evidence byte length overflows"))?;
    if n != expected {
        return Err(LockError::new(
            "E_EVIDENCE",
            "evidence length differs from crop/channel count",
        ));
    }
    if !m["file"].is_null() {
        let f = obj(&m["file"], "file")?;
        exact(f, &["sha256", "bytes"], "file")?;
        digest(&f["sha256"], "file.sha256")?;
        u(&f["bytes"], "file.bytes")?;
        if f["sha256"] != m["pcm_sha256"] || f["bytes"] != m["pcm_bytes"] {
            return Err(LockError::new(
                "E_EVIDENCE",
                "raw file evidence must equal PCM evidence",
            ));
        }
    }
    Ok(())
}
fn derive_render_input(lock: &Map<String, Value>) -> Value {
    let mut m = lock.clone();
    m.remove("render_key");
    m.remove("evidence");
    m.insert(
        "format".into(),
        Value::String("maac.generic-render-input".into()),
    );
    Value::Object(m)
}

fn verify_assets(v: &Map<String, Value>, c: &LockVerificationContext) -> Result<()> {
    let actual = arr(&v["assets"], "assets")?
        .iter()
        .map(|a| Ok((path(&a["source"], "source")?, a)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if actual.len() != c.assets.len() || !c.assets.keys().all(|k| actual.contains_key(k)) {
        return Err(LockError::new(
            "E_CLOSURE",
            "asset closure differs from resolved context",
        ));
    }
    for (k, e) in &c.assets {
        let a = obj(actual[k], "asset")?;
        if a["kind"] != e.kind
            || a["sha256"] != sha_uri(&e.bytes)
            || u(&a["bytes"], "asset.bytes")? != e.bytes.len() as u64
        {
            return Err(LockError::new(
                "E_CLOSURE",
                "asset pin differs from resolved bytes",
            ));
        }
    }
    Ok(())
}
fn verify_dependencies(v: &Map<String, Value>, c: &LockVerificationContext) -> Result<()> {
    let actual = arr(&v["dependencies"], "dependencies")?
        .iter()
        .map(|d| Ok((dep_id(d)?, d)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if actual.len() != c.dependencies.len()
        || !c.dependencies.keys().all(|k| actual.contains_key(k))
    {
        return Err(LockError::new(
            "E_CLOSURE",
            "dependency closure differs from resolved context",
        ));
    }
    for (k, b) in &c.dependencies {
        let d = obj(actual[k], "dependency")?;
        if d["sha256"] != sha_uri(b) || u(&d["bytes"], "dependency.bytes")? != b.len() as u64 {
            return Err(LockError::new(
                "E_CLOSURE",
                "dependency bytes do not match lock pin",
            ));
        }
    }
    Ok(())
}
fn verify_processors(v: &Map<String, Value>, c: &LockVerificationContext) -> Result<()> {
    let actual = arr(&v["processors"], "processors")?
        .iter()
        .map(|p| Ok((path(&p["node"], "node")?, p)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if actual.len() != c.processors.len() || !c.processors.keys().all(|k| actual.contains_key(k)) {
        return Err(LockError::new(
            "E_CLOSURE",
            "processor set differs from resolved context",
        ));
    }
    for (k, e) in &c.processors {
        let p = obj(actual[k], "processor")?;
        if p["processor_type"] != e.processor_type
            || p["implementation_hash"] != opt(&e.implementation_hash)
            || p["descriptor_hash"] != opt(&e.descriptor_hash)
            || p["adapter_id"] != opt(&e.adapter_id)
            || p["state_hash"] != opt(&e.state_hash)
            || p["config"] != e.config
            || u(&p["latency_frames"], "latency_frames")? != e.latency_frames
            || p["determinism"] != e.determinism
        {
            return Err(LockError::new(
                "E_CLOSURE",
                "processor context differs from understood contract",
            ));
        }
    }
    Ok(())
}
fn opt(v: &Option<String>) -> Value {
    v.clone().map(Value::String).unwrap_or(Value::Null)
}
fn verify_engine(v: &Map<String, Value>, c: &LockVerificationContext) -> Result<()> {
    let e = obj(&v["engine"], "engine")?;
    for (k, x) in [
        ("implementation_id", &c.engine.implementation_id),
        ("build_id", &c.engine.build_id),
        ("platform_id", &c.engine.platform_id),
        ("architecture_id", &c.engine.architecture_id),
        ("numerical_mode_id", &c.engine.numerical_mode_id),
    ] {
        if e[k] != *x {
            return Err(LockError::new(
                "E_CLOSURE",
                "engine identity differs from understood contract",
            ));
        }
    }
    if u(&e["sample_rate"], "sample_rate")? != c.engine.sample_rate {
        return Err(LockError::new(
            "E_TIMING",
            "engine sample rate differs from project",
        ));
    }
    let locked_schedule = if e["block_schedule"].is_null() {
        None
    } else {
        Some(
            arr(&e["block_schedule"], "schedule")?
                .iter()
                .map(|block| positive(block, "block"))
                .collect::<Result<Vec<_>>>()?,
        )
    };
    match (&locked_schedule, &c.engine.block_schedule) {
        (None, None) if c.engine.block_independent => {}
        (None, None) => {
            return Err(LockError::new(
                "E_CLOSURE",
                "null block schedule lacks block-independent contract",
            ));
        }
        (Some(locked), Some(expected)) if locked == expected => {}
        _ => {
            return Err(LockError::new(
                "E_CLOSURE",
                "block schedule differs from understood execution contract",
            ));
        }
    }
    if let Some(schedule) = locked_schedule {
        let total = schedule.iter().try_fold(0u64, |sum, block| {
            sum.checked_add(*block)
                .ok_or_else(|| LockError::new("E_TIMING", "block schedule overflow"))
        })?;
        if total != c.output.render_frames {
            return Err(LockError::new(
                "E_TIMING",
                "block schedule does not sum to render_frames",
            ));
        }
    }
    Ok(())
}
fn verify_output(v: &Map<String, Value>, c: &LockVerificationContext) -> Result<()> {
    let o = obj(&v["output"], "output")?;
    let crop = arr(&o["crop"], "crop")?;
    let locked_crop = (u(&crop[0], "crop")?, u(&crop[1], "crop")?);
    let locked_order = arr(&o["channel_order"], "channel_order")?
        .iter()
        .map(|x| u(x, "channel"))
        .collect::<Result<Vec<_>>>()?;
    if o["port"] != c.output.port
        || o["score"] != json!(c.output.score)
        || o["tail"] != c.output.tail
        || u(&o["render_frames"], "render_frames")? != c.output.render_frames
        || locked_crop != c.output.crop
        || locked_order != c.output.channel_order
    {
        return Err(LockError::new(
            "E_TIMING",
            "output selection differs from resolved project",
        ));
    }
    let mut permutation = locked_order;
    permutation.sort_unstable();
    if permutation != (0..c.output.channel_order.len() as u64).collect::<Vec<_>>() {
        return Err(LockError::new(
            "E_SCHEMA",
            "channel_order is not a permutation of output channels",
        ));
    }
    Ok(())
}
