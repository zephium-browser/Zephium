//! Allocation-aware JSON ingress used before deserializing durable snapshots.
//! The lexical pass allocates nothing; typed visitors then enforce collection
//! limits before pushing attacker-controlled elements into vectors.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IgnoredAny, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::{Deserialize, Deserializer};

use zephium_core::ids::ItemId;
use zephium_core::session::{MAX_SESSION_ITEMS, MAX_SPLIT_DEPTH};
use zephium_core::split::{Axis, Pane};

// Generic record URLs are capped at 8 KiB. JSON escaping can double these
// display strings, so their lexical bound remains 16 KiB. Session and Work
// records explicitly select their independently bounded ingress below.
pub(crate) const MAX_RAW_STRING_BYTES: usize = 16 * 1024;
const MAX_SCALAR_TOKEN_BYTES: usize = 256;
const MAX_STRUCTURAL_TOKENS: usize = 100_000;
const MAX_JSON_DEPTH: usize = MAX_SPLIT_DEPTH * 2 + 16;

/// Performs a conservative JSON lexical pass without decoding or allocating
/// strings. Typed deserialization remains authoritative for syntax; this pass
/// exists to establish allocation bounds before it runs.
pub(crate) fn preflight(input: &str) -> Result<PreflightedJson<'_>, &'static str> {
    preflight_with_string_limit(input, MAX_RAW_STRING_BYTES)
}

/// Session tabs preserve every URL admitted by the browser. Their complete
/// snapshot cap and typed collection/string checks remain independently enforced.
pub(crate) fn preflight_session(input: &str) -> Result<PreflightedJson<'_>, &'static str> {
    preflight_with_string_limit(input, 2 * zephium_core::navigation::MAX_URL_BYTES)
}

/// Work execution rows are independently capped at 512 KiB before this pass.
/// Their semantic artifact text permits 32 KiB, with at most 2x JSON escaping
/// from the canonical writer. Generic record ingress keeps its smaller bound.
pub(crate) fn preflight_work_execution(input: &str) -> Result<PreflightedJson<'_>, &'static str> {
    preflight_with_string_limit(
        input,
        2 * zephium_core::work::artifact::MAX_ARTIFACT_TEXT_BYTES,
    )
}

fn preflight_with_string_limit(
    input: &str,
    max_string_bytes: usize,
) -> Result<PreflightedJson<'_>, &'static str> {
    let bytes = input.as_bytes();
    let mut stack = [0_u8; MAX_JSON_DEPTH];
    let mut depth = 0_usize;
    let mut structural = 0_usize;
    let mut index = 0_usize;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                let mut raw_bytes = 0_usize;
                let mut closed = false;
                while index < bytes.len() {
                    let byte = bytes[index];
                    if byte == b'"' {
                        index += 1;
                        closed = true;
                        break;
                    }
                    if byte < 0x20 {
                        return Err("JSON string contains an unescaped control byte");
                    }
                    if byte == b'\\' {
                        if index + 1 >= bytes.len() {
                            return Err("JSON string ends in an escape");
                        }
                        raw_bytes = raw_bytes.saturating_add(2);
                        index += 2;
                    } else {
                        raw_bytes = raw_bytes.saturating_add(1);
                        index += 1;
                    }
                    if raw_bytes > max_string_bytes {
                        return Err("JSON string exceeds allocation limit");
                    }
                }
                if !closed {
                    return Err("unterminated JSON string");
                }
            }
            open @ (b'{' | b'[') => {
                structural = structural.saturating_add(1);
                if structural > MAX_STRUCTURAL_TOKENS || depth == MAX_JSON_DEPTH {
                    return Err("JSON structure exceeds allocation limit");
                }
                stack[depth] = open;
                depth += 1;
                index += 1;
            }
            close @ (b'}' | b']') => {
                structural = structural.saturating_add(1);
                if structural > MAX_STRUCTURAL_TOKENS || depth == 0 {
                    return Err("invalid or excessive JSON structure");
                }
                depth -= 1;
                let expected = if stack[depth] == b'{' { b'}' } else { b']' };
                if close != expected {
                    return Err("mismatched JSON structure");
                }
                index += 1;
            }
            b',' | b':' => {
                structural = structural.saturating_add(1);
                if structural > MAX_STRUCTURAL_TOKENS {
                    return Err("JSON structure exceeds allocation limit");
                }
                index += 1;
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'"' | b'{' | b'}' | b'[' | b']' | b',' | b':')
                {
                    index += 1;
                    if index - start > MAX_SCALAR_TOKEN_BYTES {
                        return Err("JSON scalar exceeds allocation limit");
                    }
                }
            }
        }
    }
    if depth != 0 {
        return Err("unterminated JSON structure");
    }
    Ok(PreflightedJson { input })
}

/// Proof that the fixed-stack lexical limits ran successfully. Keeping the
/// input private makes it impossible for another store module to disable
/// Serde's recursion limit without first establishing the outer depth/token
/// bounds.
pub(crate) struct PreflightedJson<'a> {
    input: &'a str,
}

impl PreflightedJson<'_> {
    pub(crate) fn deserialize<T>(&self) -> Result<T, &'static str>
    where
        T: DeserializeOwned,
    {
        let mut deserializer = serde_json::Deserializer::from_str(self.input);
        deserializer.disable_recursion_limit();
        let value = T::deserialize(&mut deserializer).map_err(|_| "invalid bounded JSON")?;
        deserializer
            .end()
            .map_err(|_| "trailing bounded JSON data")?;
        Ok(value)
    }
}

/// Deserializes only after the allocation-free lexical limits have passed.
/// Serde JSON's built-in recursion limit is disabled because the recursive
/// fields below have stricter schema-aware depth counters of their own. This
/// lets a snapshot at Zephium's documented split-depth limit round-trip while
/// keeping arbitrary JSON nesting bounded by `preflight`.
pub(crate) fn from_str<T>(input: &str) -> Result<T, &'static str>
where
    T: DeserializeOwned,
{
    preflight(input)?.deserialize()
}

pub(crate) fn deserialize_bounded_vec<'de, D, T>(
    deserializer: D,
    limit: usize,
    valid: fn(&T) -> bool,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedVecVisitor<T> {
        limit: usize,
        valid: fn(&T) -> bool,
        marker: PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for BoundedVecVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "an array with at most {} bounded elements",
                self.limit
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            if sequence.size_hint().is_some_and(|size| size > self.limit) {
                return Err(de::Error::custom("array exceeds persistence limit"));
            }
            let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(self.limit));
            for _ in 0..self.limit {
                let Some(value) = sequence.next_element::<T>()? else {
                    return Ok(values);
                };
                if !(self.valid)(&value) {
                    return Err(de::Error::custom("array element exceeds persistence limit"));
                }
                values.push(value);
            }
            // Probe one additional value without materializing it. An error
            // stops the parser immediately instead of allocating element N+1.
            if sequence.next_element::<IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("array exceeds persistence limit"));
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(BoundedVecVisitor {
        limit,
        valid,
        marker: PhantomData,
    })
}

/// Authoritative split decoder with shared depth/leaf counters. This avoids
/// first allocating an arbitrary recursive `Pane` and validating it later.
pub(crate) struct BoundedPane(pub(crate) Pane);

impl<'de> Deserialize<'de> for BoundedPane {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut leaves = 0_usize;
        PaneSeed {
            depth: 0,
            leaves: &mut leaves,
        }
        .deserialize(deserializer)
        .map(BoundedPane)
    }
}

struct PaneSeed<'a> {
    depth: usize,
    leaves: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for PaneSeed<'_> {
    type Value = Pane;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.depth > MAX_SPLIT_DEPTH {
            return Err(de::Error::custom("split tree exceeds depth limit"));
        }
        deserializer.deserialize_enum("Pane", &["Leaf", "Branch"], PaneVisitor { seed: self })
    }
}

struct PaneVisitor<'a> {
    seed: PaneSeed<'a>,
}

#[derive(Deserialize)]
#[serde(field_identifier)]
enum PaneVariant {
    Leaf,
    Branch,
}

impl<'de> Visitor<'de> for PaneVisitor<'_> {
    type Value = Pane;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded split pane")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: EnumAccess<'de>,
    {
        let (variant, access) = data.variant::<PaneVariant>()?;
        match variant {
            PaneVariant::Leaf => {
                if *self.seed.leaves == MAX_SESSION_ITEMS {
                    return Err(de::Error::custom("split tree exceeds leaf limit"));
                }
                let id = access.newtype_variant::<ItemId>()?;
                *self.seed.leaves += 1;
                Ok(Pane::Leaf(id))
            }
            PaneVariant::Branch => access.struct_variant(
                &["axis", "ratio", "a", "b"],
                BranchVisitor { seed: self.seed },
            ),
        }
    }
}

struct BranchVisitor<'a> {
    seed: PaneSeed<'a>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum BranchField {
    Axis,
    Ratio,
    A,
    B,
}

impl<'de> Visitor<'de> for BranchVisitor<'_> {
    type Value = Pane;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded split branch")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut axis = None;
        let mut ratio = None;
        let mut first = None;
        let mut second = None;
        while let Some(field) = map.next_key::<BranchField>()? {
            match field {
                BranchField::Axis => {
                    if axis.replace(map.next_value::<Axis>()?).is_some() {
                        return Err(de::Error::duplicate_field("axis"));
                    }
                }
                BranchField::Ratio => {
                    if ratio.replace(map.next_value::<f64>()?).is_some() {
                        return Err(de::Error::duplicate_field("ratio"));
                    }
                }
                BranchField::A => {
                    if first.is_some() {
                        return Err(de::Error::duplicate_field("a"));
                    }
                    first = Some(Box::new(map.next_value_seed(PaneSeed {
                        depth: self.seed.depth + 1,
                        leaves: self.seed.leaves,
                    })?));
                }
                BranchField::B => {
                    if second.is_some() {
                        return Err(de::Error::duplicate_field("b"));
                    }
                    second = Some(Box::new(map.next_value_seed(PaneSeed {
                        depth: self.seed.depth + 1,
                        leaves: self.seed.leaves,
                    })?));
                }
            }
        }
        let ratio = ratio.ok_or_else(|| de::Error::missing_field("ratio"))?;
        if !ratio.is_finite() || !(0.05..=0.95).contains(&ratio) {
            return Err(de::Error::custom(
                "split ratio is outside persistence limits",
            ));
        }
        Ok(Pane::Branch {
            axis: axis.ok_or_else(|| de::Error::missing_field("axis"))?,
            ratio,
            a: first.ok_or_else(|| de::Error::missing_field("a"))?,
            b: second.ok_or_else(|| de::Error::missing_field("b"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skewed_pane(depth: usize, next: &mut u128) -> Pane {
        let first = Pane::Leaf(ItemId::from(*next));
        *next += 1;
        if depth == 0 {
            first
        } else {
            Pane::Branch {
                axis: Axis::Row,
                ratio: 0.5,
                a: Box::new(first),
                b: Box::new(skewed_pane(depth - 1, next)),
            }
        }
    }

    fn pane_with_leaves(leaves: usize, next: &mut u128) -> Pane {
        if leaves == 1 {
            let pane = Pane::Leaf(ItemId::from(*next));
            *next += 1;
            return pane;
        }
        let first = leaves / 2;
        Pane::Branch {
            axis: Axis::Col,
            ratio: 0.5,
            a: Box::new(pane_with_leaves(first, next)),
            b: Box::new(pane_with_leaves(leaves - first, next)),
        }
    }

    #[test]
    fn work_text_ingress_does_not_expand_generic_record_string_admission() {
        let text = "\t".repeat(zephium_core::work::artifact::MAX_ARTIFACT_TEXT_BYTES);
        let body = serde_json::to_string(&text).unwrap();
        assert!(preflight_work_execution(&body).is_ok());
        assert!(preflight(&body).is_err());
        let oversized = serde_json::to_string(&(text + "\t")).unwrap();
        assert!(preflight_work_execution(&oversized).is_err());
    }

    #[test]
    fn browser_session_url_admission_does_not_expand_generic_record_tokens() {
        let long =
            serde_json::to_string(&format!("https://example.test/#{}", "x".repeat(32 * 1024)))
                .unwrap();
        assert!(preflight_session(&long).is_ok());
        assert!(preflight(&long).is_err());
        assert!(preflight_session(
            &serde_json::to_string(&"x".repeat(2 * zephium_core::navigation::MAX_URL_BYTES + 1))
                .unwrap()
        )
        .is_err());
    }

    #[test]
    fn lexical_preflight_rejects_large_tokens_and_deep_structure() {
        assert!(preflight(&format!(
            r#"{{"x":"{}"}}"#,
            "x".repeat(MAX_RAW_STRING_BYTES)
        ))
        .is_ok());
        assert!(preflight(&format!(
            r#"{{"x":"{}"}}"#,
            "x".repeat(MAX_RAW_STRING_BYTES + 1)
        ))
        .is_err());
        let deeply_nested = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert!(preflight(&deeply_nested).is_err());
    }

    #[test]
    fn schema_aware_split_parser_accepts_exact_depth_and_rejects_more() {
        let mut next = 1;
        let maximum = skewed_pane(MAX_SPLIT_DEPTH, &mut next);
        let json = serde_json::to_string(&maximum).unwrap();
        let decoded = from_str::<BoundedPane>(&json).unwrap();
        assert_eq!(decoded.0, maximum);

        let too_deep = skewed_pane(MAX_SPLIT_DEPTH + 1, &mut next);
        let json = serde_json::to_string(&too_deep).unwrap();
        assert!(from_str::<BoundedPane>(&json).is_err());
    }

    #[test]
    fn schema_aware_split_parser_rejects_too_many_leaves_during_parse() {
        let mut next = 1;
        let pane = pane_with_leaves(MAX_SESSION_ITEMS + 1, &mut next);
        let json = serde_json::to_string(&pane).unwrap();
        assert!(from_str::<BoundedPane>(&json).is_err());
    }
}
