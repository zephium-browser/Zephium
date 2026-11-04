// Binds page code to the real site: the frame's host and the top-level page's host, read the way the pause check reads it and compared as text, never by the host table's hash.
function zephiumPageCodeSite(sites) {
    const under = (name, site) => name === site || name.endsWith(`.${site}`);
    const host = location.hostname.replace(/\.$/, '');
    let top = host;
    const origins = location.ancestorOrigins;
    if (origins && origins.length > 0) {
        try {
            top = new URL(origins[origins.length - 1]).hostname.replace(/\.$/, '');
        } catch (e) {
            return false;
        }
    }
    return sites.some((site) => under(host, site) && under(top, site));
}
