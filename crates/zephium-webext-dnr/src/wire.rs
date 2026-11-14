use crate::index::IndexError;
use crate::model::*;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], IndexError> {
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
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError>;
}
macro_rules! number {
    ($($ty:ty),* $(,)?) => {$(impl Wire for $ty {
        fn write(&self, bytes: &mut Vec<u8>) { bytes.extend_from_slice(&self.to_le_bytes()); }
        fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> { Ok(Self::from_le_bytes(reader.take(std::mem::size_of::<Self>())?.try_into().map_err(|_| invalid())?)) }
    })*};
}
number!(u8, u32, i32, u64);
impl Wire for bool {
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
    fn write(&self, bytes: &mut Vec<u8>) {
        (self.len() as u32).write(bytes);
        bytes.extend_from_slice(self.as_bytes());
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        let length = u32::read(reader)? as usize;
        std::str::from_utf8(reader.take(length)?)
            .map(str::to_owned)
            .map_err(|_| invalid())
    }
}
impl<T: Wire> Wire for Box<T> {
    fn write(&self, bytes: &mut Vec<u8>) {
        self.as_ref().write(bytes);
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        Ok(Box::new(T::read(reader)?))
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn write(&self, bytes: &mut Vec<u8>) {
        (self.len() as u32).write(bytes);
        for item in self {
            item.write(bytes);
        }
    }
    fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> {
        let count = u32::read(reader)? as usize;
        // Every supported element occupies at least one byte. Never reserve an
        // attacker-declared count before proving its bytes are present.
        if count > reader.bytes.len() - reader.offset || count > MAX_RULES_PER_RULESET {
            return Err(invalid());
        }
        let mut result = Vec::new();
        for _ in 0..count {
            result.push(T::read(reader)?);
        }
        Ok(result)
    }
}
impl<T: Wire> Wire for Option<T> {
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
macro_rules! structure {
    ($ty:ident { $($field:ident),* $(,)? }) => {impl Wire for $ty {
        fn write(&self, bytes: &mut Vec<u8>) { $(self.$field.write(bytes);)* }
        fn read(reader: &mut Reader<'_>) -> Result<Self, IndexError> { Ok(Self { $($field: Wire::read(reader)?,)* }) }
    }};
}
macro_rules! enumeration {
    ($ty:ident { $($tag:literal => $variant:ident),* $(,)? }) => {impl Wire for $ty {
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
structure!(HeaderCondition {
    header,
    values,
    excluded_values
});
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
