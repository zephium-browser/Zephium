use serde_json::{Number, Value};

use super::{Area, Changes, Items, Mutation, StorageChange, StorageError};

/// Chrome sync's maximum total quota bytes.
pub const QUOTA_BYTES: u64 = 102_400;
/// Chrome sync's maximum bytes for a key and its serialized value.
pub const QUOTA_BYTES_PER_ITEM: u64 = 8_192;
/// Chrome sync's maximum number of properties.
pub const MAX_ITEMS: usize = 512;
/// Chrome sync's short-term write-operation limit.
pub const MAX_WRITE_OPERATIONS_PER_MINUTE: u32 = 120;
/// Chrome sync's long-term write-operation limit.
pub const MAX_WRITE_OPERATIONS_PER_HOUR: u32 = 1_800;
/// Deprecated Chrome constant; no sustained-write quota is enforced.
pub const MAX_SUSTAINED_WRITE_OPERATIONS_PER_MINUTE: u32 = 1_000_000;

const MINUTE: u64 = 60_000_000_000;
const HOUR: u64 = 60 * MINUTE;

/// UTF-8 key bytes plus Chromium's compact JSON value size. The top-level
/// key has no quotes; nested keys do. Chromium also escapes `<`, U+2028 and
/// U+2029. Session uses the same reproducible accounting in this runtime.
pub fn item_bytes(key: &str, value: &Value) -> u64 {
    key.len() as u64 + json_bytes(value)
}

fn string_bytes(value: &str) -> u64 {
    2 + value
        .chars()
        .map(|character| match character {
            '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
            '\u{0}'..='\u{1f}' | '<' | '\u{2028}' | '\u{2029}' => 6,
            _ => character.len_utf8() as u64,
        })
        .sum::<u64>()
}

fn json_bytes(value: &Value) -> u64 {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::String(value) => string_bytes(value),
        Value::Number(number) => number_bytes(number),
        Value::Array(values) => {
            2 + values.len().saturating_sub(1) as u64 + values.iter().map(json_bytes).sum::<u64>()
        }
        Value::Object(values) => {
            2 + values.len().saturating_sub(1) as u64
                + values
                    .iter()
                    .map(|(key, value)| string_bytes(key) + 1 + json_bytes(value))
                    .sum::<u64>()
        }
    }
}

fn number_bytes(number: &Number) -> u64 {
    if let Some(integer) = number
        .as_i64()
        .filter(|value| i32::try_from(*value).is_ok())
    {
        return integer.to_string().len() as u64;
    }
    // Chromium preserves its double type with `.0` and uses ECMAScript's
    // decimal/exponent thresholds. Wire integers outside i32 become doubles.
    let real = number.as_f64().unwrap_or(0.0);
    if real == 0.0 {
        return if real.is_sign_negative() { 4 } else { 3 };
    }
    let text = real.abs().to_string();
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text.as_str(), 0), |(mantissa, power)| {
            (mantissa, power.parse::<i32>().unwrap_or(0))
        });
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let mut digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let leading_zeroes = digits.len() - digits.trim_start_matches('0').len();
    digits.drain(..leading_zeroes);
    let point = decimal + exponent - leading_zeroes as i32;
    while digits.ends_with('0') && digits.len() > 1 {
        digits.pop();
    }
    let power = point - 1;
    let sign = u64::from(real.is_sign_negative());
    if (-6..21).contains(&power) {
        if point <= 0 {
            sign + 2 + (-point) as u64 + digits.len() as u64
        } else if point as usize >= digits.len() {
            sign + point as u64 + 2
        } else {
            sign + digits.len() as u64 + 1
        }
    } else {
        sign + digits.len() as u64
            + u64::from(digits.len() > 1)
            + 2
            + power.unsigned_abs().to_string().len() as u64
    }
}

pub(super) fn prepare(
    items: &Items,
    mutation: &Mutation,
    area: Area,
    maximum: u64,
) -> Result<Changes, StorageError> {
    let mut changes = Changes::new();
    match mutation {
        Mutation::Set(values) => {
            for (key, value) in values {
                if area == Area::Sync && item_bytes(key, value) > QUOTA_BYTES_PER_ITEM {
                    return Err(StorageError::QuotaExceeded("Resource::kQuotaBytesPerItem"));
                }
                if items.get(key) != Some(value) {
                    changes.insert(
                        key.clone(),
                        StorageChange {
                            old_value: items.get(key).cloned(),
                            new_value: Some(value.clone()),
                        },
                    );
                }
            }
        }
        Mutation::Remove(keys) => {
            for key in keys {
                if let Some(value) = items.get(key) {
                    changes.insert(
                        key.clone(),
                        StorageChange {
                            old_value: Some(value.clone()),
                            new_value: None,
                        },
                    );
                }
            }
        }
        Mutation::Clear => {
            changes.extend(items.iter().map(|(key, value)| {
                (
                    key.clone(),
                    StorageChange {
                        old_value: Some(value.clone()),
                        new_value: None,
                    },
                )
            }));
        }
    }
    if matches!(mutation, Mutation::Set(_)) {
        let mut bytes: u64 = items
            .iter()
            .map(|(key, value)| item_bytes(key, value))
            .sum();
        let mut count = items.len();
        for (key, change) in &changes {
            if let Some(old) = &change.old_value {
                bytes -= item_bytes(key, old);
            } else {
                count += 1;
            }
            if let Some(new) = &change.new_value {
                bytes = bytes.saturating_add(item_bytes(key, new));
            }
        }
        if bytes > maximum {
            return Err(StorageError::QuotaExceeded("Resource::kQuotaBytes"));
        }
        if area == Area::Sync && count > MAX_ITEMS {
            return Err(StorageError::QuotaExceeded("Resource::kMaxItems"));
        }
    }
    Ok(changes)
}

#[derive(Default)]
pub(super) struct WriteRate {
    minute: Window,
    hour: Window,
}

impl WriteRate {
    pub(super) fn charge(&mut self, now: u64) -> Result<(), StorageError> {
        self.minute.charge(
            now,
            MINUTE,
            MAX_WRITE_OPERATIONS_PER_MINUTE,
            "MAX_WRITE_OPERATIONS_PER_MINUTE",
        )?;
        self.hour.charge(
            now,
            HOUR,
            MAX_WRITE_OPERATIONS_PER_HOUR,
            "MAX_WRITE_OPERATIONS_PER_HOUR",
        )
    }
}

#[derive(Default)]
struct Window {
    start: Option<u64>,
    used: u32,
}

impl Window {
    fn charge(
        &mut self,
        now: u64,
        span: u64,
        maximum: u32,
        resource: &'static str,
    ) -> Result<(), StorageError> {
        if self
            .start
            .is_none_or(|start| now.saturating_sub(start) > span)
        {
            self.start = Some(now);
            self.used = 0;
        }
        if self.used >= maximum {
            return Err(StorageError::WriteRateExceeded(resource));
        }
        self.used += 1;
        Ok(())
    }
}
