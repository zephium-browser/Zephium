use crate::policy::same_origin::same_origin;
use url::Url;

/// True when `candidate` remains inside an installed app's scope. A path
/// prefix is honoured only on a path-segment boundary, so a scope of `/mail`
/// never captures `/mailer`.
pub fn is_in_scope(candidate: &Url, scope: &Url) -> bool {
    if !same_origin(candidate, scope) {
        return false;
    }
    let scope_path = scope.path();
    let candidate_path = candidate.path();
    if scope_path == "/" {
        return true;
    }
    if scope_path.ends_with('/') {
        return candidate_path.starts_with(scope_path);
    }
    candidate_path == scope_path
        || candidate_path
            .strip_prefix(scope_path)
            .is_some_and(|rest| rest.starts_with('/'))
}
