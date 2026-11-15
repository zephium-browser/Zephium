use crate::index::IndexError;
use crate::model::*;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    decoded_bytes: usize,
    decoded_limit: usize,
    header_patterns: usize,
}
impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            decoded_bytes: 0,
            decoded_limit: MAX_DECODED_ACTION_BYTES_PER_RULESET,
            header_patterns: 0,
        }
    }
    pub(crate) fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }
    pub(crate) fn header_pattern_count(&self) -> usize {
        self.header_patterns
    }
    fn charge_decoded(&mut self, bytes: usize) -> Result<(), IndexError> {
        self.decoded_bytes = self
            .decoded_bytes
            .checked_add(bytes)
            .filter(|&n| n <= self.decoded_limit)
            .ok_or_else(|| IndexError("decoded action data exceeds byte limit".into()))?;
        Ok(())
    }
    fn count(&mut self) -> Result<usize, IndexError> {
        let count = u32::read(self)? as usize;
        if count > self.bytes.len() - self.offset || count > MAX_RULES_PER_RULESET {
            return Err(invalid());
        }
        Ok(count)
    }
    fn string(&mut self, limit: usize) -> Result<&'a str, IndexError> {
        let length = u32::read(self)? as usize;
        if length > limit {
            return Err(invalid());
        }
        let bytes = self.take(length)?;
        self.charge_decoded(length)?;
        std::str::from_utf8(bytes).map_err(|_| invalid())
    }
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], IndexError> {
        let end = self.offset.checked_add(count).ok_or_else(invalid)?;
        let result = self.bytes.get(self.offset..end).ok_or_else(invalid)?;
        self.offset = end;
        Ok(result)
    }
    pub(crate) fn finish(self) -> Result<(), IndexError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
fn invalid() -> IndexError {
    IndexError("invalid binary rule encoding".into())
}
pub(crate) trait Wire: Sized {
    fn write(&self, bytes: &mut Vec<u8>);
    fn encoded_len(&self) -> Option<usize>;
    fn heap_len(&self) -> Option<usize>;
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError>;
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        Self::read(reader).map(|_| ())
    }
}
macro_rules! number {
    ($($ty:ty),* $(,)?) => {$(impl Wire for $ty {
        fn encoded_len(&self) -> Option<usize> { Some(std::mem::size_of::<Self>()) }
        fn heap_len(&self) -> Option<usize> { Some(0) }
        fn write(&self, bytes: &mut Vec<u8>) { bytes.extend_from_slice(&self.to_le_bytes()); }
        fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> { Ok(Self::from_le_bytes(reader.take(std::mem::size_of::<Self>())?.try_into().map_err(|_| invalid())?)) }
    })*};
}
number!(u8, u32, i32, u64);
impl Wire for bool {
    fn heap_len(&self) -> Option<usize> {
        Some(0)
    }
    fn encoded_len(&self) -> Option<usize> {
        Some(1)
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        u8::from(*self).write(bytes);
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        match u8::read(reader)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
}
impl Wire for String {
    fn heap_len(&self) -> Option<usize> {
        Some(self.len())
    }
    fn encoded_len(&self) -> Option<usize> {
        u32::try_from(self.len()).ok()?;
        self.len().checked_add(4)
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        reader.string(usize::MAX).map(|_| ())
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        (self.len() as u32).write(bytes);
        bytes.extend_from_slice(self.as_bytes());
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        reader.string(usize::MAX).map(str::to_owned)
    }
}
impl<T: Wire> Wire for Box<T> {
    fn heap_len(&self) -> Option<usize> {
        std::mem::size_of::<T>().checked_add(self.as_ref().heap_len()?)
    }
    fn encoded_len(&self) -> Option<usize> {
        self.as_ref().encoded_len()
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        reader.charge_decoded(std::mem::size_of::<T>())?;
        T::validate(reader)
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        self.as_ref().write(bytes);
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        reader.charge_decoded(std::mem::size_of::<T>())?;
        Ok(Box::new(T::read(reader)?))
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn heap_len(&self) -> Option<usize> {
        self.iter().try_fold(
            self.len().checked_mul(std::mem::size_of::<T>())?,
            |size, value| size.checked_add(value.heap_len()?),
        )
    }
    fn encoded_len(&self) -> Option<usize> {
        u32::try_from(self.len()).ok()?;
        self.iter()
            .try_fold(4usize, |len, value| len.checked_add(value.encoded_len()?))
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        let count = reader.count()?;
        reader.charge_decoded(
            count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(invalid)?,
        )?;
        for _ in 0..count {
            T::validate(reader)?;
        }
        Ok(())
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        (self.len() as u32).write(bytes);
        for item in self {
            item.write(bytes);
        }
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        let count = reader.count()?;
        reader.charge_decoded(
            count
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(invalid)?,
        )?;
        let mut result = Vec::new();
        result.try_reserve_exact(count).map_err(|_| invalid())?;
        for _ in 0..count {
            result.push(T::read(reader)?);
        }
        Ok(result)
    }
}
impl<T: Wire> Wire for Option<T> {
    fn heap_len(&self) -> Option<usize> {
        self.as_ref().map_or(Some(0), Wire::heap_len)
    }
    fn encoded_len(&self) -> Option<usize> {
        1usize.checked_add(self.as_ref().map_or(Some(0), Wire::encoded_len)?)
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        if bool::read(reader)? {
            T::validate(reader)?;
        }
        Ok(())
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        self.is_some().write(bytes);
        if let Some(value) = self {
            value.write(bytes);
        }
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        if bool::read(reader)? {
            Ok(Some(T::read(reader)?))
        } else {
            Ok(None)
        }
    }
}
fn validate_field<S, T: Wire>(
    reader: &mut Reader<'_>,
    _: impl Fn(&S) -> &T,
) -> Result<(), IndexError> {
    T::validate(reader)
}
macro_rules! structure {
    ($ty:ident { $($field:ident),* $(,)? }) => {impl Wire for $ty {
        fn heap_len(&self) -> Option<usize> { let mut len=0usize; $(len=len.checked_add(self.$field.heap_len()?)?;)* Some(len) }
        fn encoded_len(&self) -> Option<usize> { let mut len=0usize; $(len=len.checked_add(self.$field.encoded_len()?)?;)* Some(len) }
        fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> { $(validate_field::<$ty, _>(reader, |v| &v.$field)?;)* Ok(()) }
        fn write(&self, bytes: &mut Vec<u8>) { $(self.$field.write(bytes);)* }
        fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> { Ok(Self { $($field: Wire::read(reader)?,)* }) }
    }};
}
macro_rules! enumeration {
    ($ty:ident { $($tag:literal => $variant:ident),* $(,)? }) => {impl Wire for $ty {
        fn heap_len(&self) -> Option<usize> { Some(0) }
        fn encoded_len(&self) -> Option<usize> { Some(1) }
        fn write(&self, bytes: &mut Vec<u8>) { match self { $(Self::$variant => ($tag as u8).write(bytes),)* } }
        fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> { match u8::read(reader)? { $($tag => Ok(Self::$variant),)* _ => Err(invalid()) } }
    }};
}
enumeration!(RulesetKind { 0 => Static, 1 => Dynamic, 2 => Session });
enumeration!(DomainType { 0 => FirstParty, 1 => ThirdParty });
enumeration!(ResourceType { 0 => MainFrame, 1 => SubFrame, 2 => Stylesheet, 3 => Script, 4 => Image, 5 => Font, 6 => Object, 7 => Xmlhttprequest, 8 => Ping, 9 => CspReport, 10 => Media, 11 => Websocket, 12 => Webtransport, 13 => Webbundle, 14 => Other });
enumeration!(RequestMethod { 0 => Connect, 1 => Delete, 2 => Get, 3 => Head, 4 => Options, 5 => Patch, 6 => Post, 7 => Put, 8 => Other });
enumeration!(HeaderOperation { 0 => Append, 1 => Set, 2 => Remove });
structure!(Rule {
    id,
    priority,
    action,
    condition
});
structure!(Condition {
    url_filter,
    regex_filter,
    is_url_filter_case_sensitive,
    initiator_domains,
    excluded_initiator_domains,
    request_domains,
    excluded_request_domains,
    top_domains,
    excluded_top_domains,
    domains,
    excluded_domains,
    resource_types,
    excluded_resource_types,
    request_methods,
    excluded_request_methods,
    domain_type,
    tab_ids,
    excluded_tab_ids,
    response_headers,
    excluded_response_headers
});
structure!(HeaderModification {
    header,
    operation,
    value
});
fn header_values(reader: &mut Reader<'_>, decode: bool) -> Result<Option<Vec<String>>, IndexError> {
    if !bool::read(reader)? {
        return Ok(None);
    }
    let count = reader.count()?;
    reader.header_patterns = reader
        .header_patterns
        .checked_add(count)
        .filter(|&n| n <= MAX_HEADER_PATTERNS_PER_RULE)
        .ok_or_else(|| IndexError("MAX_HEADER_PATTERNS_PER_RULE exceeded".into()))?;
    reader.charge_decoded(
        count
            .checked_mul(std::mem::size_of::<String>())
            .ok_or_else(invalid)?,
    )?;
    let mut values = Vec::new();
    if decode {
        values.try_reserve_exact(count).map_err(|_| invalid())?;
    }
    for _ in 0..count {
        let value = reader.string(MAX_HEADER_PATTERN_BYTES)?;
        if decode {
            values.push(value.to_owned());
        }
    }
    Ok(Some(values))
}
impl Wire for HeaderCondition {
    fn encoded_len(&self) -> Option<usize> {
        self.header
            .encoded_len()?
            .checked_add(self.values.encoded_len()?)?
            .checked_add(self.excluded_values.encoded_len()?)
    }
    fn heap_len(&self) -> Option<usize> {
        self.header
            .heap_len()?
            .checked_add(self.values.heap_len()?)?
            .checked_add(self.excluded_values.heap_len()?)
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        self.header.write(bytes);
        self.values.write(bytes);
        self.excluded_values.write(bytes);
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        String::validate(reader)?;
        header_values(reader, false)?;
        header_values(reader, false)?;
        Ok(())
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        let header = reader.string(usize::MAX)?;
        let values = header_values(reader, true)?;
        let excluded_values = header_values(reader, true)?;
        Ok(Self {
            header: header.to_owned(),
            values,
            excluded_values,
        })
    }
}
structure!(Redirect {
    url,
    extension_path,
    regex_substitution,
    transform
});
structure!(UrlTransform {
    scheme,
    host,
    port,
    path,
    query,
    query_transform,
    fragment,
    username,
    password
});
structure!(QueryTransform {
    remove_params,
    add_or_replace_params
});
structure!(QueryKeyValue {
    key,
    value,
    replace_only
});
impl Wire for Action {
    fn heap_len(&self) -> Option<usize> {
        match self {
            Self::Block | Self::Allow | Self::AllowAllRequests | Self::UpgradeScheme => Some(0),
            Self::Redirect { redirect } => redirect.heap_len(),
            Self::ModifyHeaders {
                request_headers,
                response_headers,
            } => request_headers
                .heap_len()?
                .checked_add(response_headers.heap_len()?),
        }
    }
    fn encoded_len(&self) -> Option<usize> {
        match self {
            Self::Block | Self::Allow | Self::AllowAllRequests | Self::UpgradeScheme => Some(1),
            Self::Redirect { redirect } => 1usize.checked_add(redirect.encoded_len()?),
            Self::ModifyHeaders {
                request_headers,
                response_headers,
            } => 1usize
                .checked_add(request_headers.encoded_len()?)?
                .checked_add(response_headers.encoded_len()?),
        }
    }
    fn validate(reader: &mut Reader<'_>) -> Result<(), IndexError> {
        match u8::read(reader)? {
            0..=3 => Ok(()),
            4 => Box::<Redirect>::validate(reader),
            5 => {
                Vec::<HeaderModification>::validate(reader)?;
                Vec::<HeaderModification>::validate(reader)
            }
            _ => Err(invalid()),
        }
    }
    fn write(&self, bytes: &mut Vec<u8>) {
        match self {
            Self::Block => 0u8.write(bytes),
            Self::Allow => 1u8.write(bytes),
            Self::AllowAllRequests => 2u8.write(bytes),
            Self::UpgradeScheme => 3u8.write(bytes),
            Self::Redirect { redirect } => {
                4u8.write(bytes);
                redirect.write(bytes);
            }
            Self::ModifyHeaders {
                request_headers,
                response_headers,
            } => {
                5u8.write(bytes);
                request_headers.write(bytes);
                response_headers.write(bytes);
            }
        }
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        match u8::read(reader)? {
            0 => Ok(Self::Block),
            1 => Ok(Self::Allow),
            2 => Ok(Self::AllowAllRequests),
            3 => Ok(Self::UpgradeScheme),
            4 => Ok(Self::Redirect {
                redirect: Wire::read(reader)?,
            }),
            5 => Ok(Self::ModifyHeaders {
                request_headers: Wire::read(reader)?,
                response_headers: Wire::read(reader)?,
            }),
            _ => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(bytes: &[u8], limit: usize) -> Reader<'_> {
        let mut reader = Reader::new(bytes);
        reader.decoded_limit = limit;
        reader
    }

    #[test]
    fn header_only_vectors_are_admitted_before_backing_allocation() {
        let values = vec![
            HeaderCondition {
                header: "x".into(),
                values: None,
                excluded_values: None
            };
            2
        ];
        let mut bytes = Vec::new();
        values.write(&mut bytes);
        let limit = 2 * std::mem::size_of::<HeaderCondition>() - 1;
        let mut validating = reader(&bytes, limit);
        assert!(Vec::<HeaderCondition>::validate(&mut validating).is_err());
        assert_eq!(validating.decoded_bytes(), 0);
        let mut decoding = reader(&bytes, limit);
        assert!(Vec::<HeaderCondition>::read(&mut decoding).is_err());
        assert_eq!(decoding.decoded_bytes(), 0);
    }

    #[test]
    fn vector_backing_and_child_strings_share_one_budget() {
        let values = vec![String::from("abc"), String::from("de")];
        let mut bytes = Vec::new();
        values.write(&mut bytes);
        let required = values.heap_len().unwrap();
        assert!(Vec::<String>::validate(&mut reader(&bytes, required - 1)).is_err());
        assert!(Vec::<String>::read(&mut reader(&bytes, required - 1)).is_err());
        let mut exact = reader(&bytes, required);
        assert_eq!(Vec::<String>::read(&mut exact).unwrap(), values);
        assert_eq!(exact.decoded_bytes(), required);
    }

    #[test]
    fn header_pattern_counts_and_lengths_are_checked_before_cloning_values() {
        for values in [
            vec![String::new(); MAX_HEADER_PATTERNS_PER_RULE + 1],
            vec!["x".repeat(MAX_HEADER_PATTERN_BYTES + 1)],
        ] {
            let header = HeaderCondition {
                header: "x".into(),
                values: Some(values),
                excluded_values: None,
            };
            let mut bytes = Vec::new();
            header.write(&mut bytes);
            let mut validating = Reader::new(&bytes);
            assert!(HeaderCondition::validate(&mut validating).is_err());
            assert!(validating.decoded_bytes() <= 1 + std::mem::size_of::<String>());
            let mut decoding = Reader::new(&bytes);
            assert!(HeaderCondition::read(&mut decoding).is_err());
            assert!(decoding.decoded_bytes() <= 1 + std::mem::size_of::<String>());
        }
        let header = HeaderCondition {
            header: "x".into(),
            values: Some(vec![String::new(); MAX_HEADER_PATTERNS_PER_RULE]),
            excluded_values: Some(vec![String::new()]),
        };
        let mut bytes = Vec::new();
        header.write(&mut bytes);
        assert!(HeaderCondition::validate(&mut Reader::new(&bytes)).is_err());
        assert!(HeaderCondition::read(&mut Reader::new(&bytes)).is_err());
    }
}
