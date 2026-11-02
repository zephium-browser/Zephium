//! Files the person takes out of the browser: bookmarks as the Netscape HTML
//! every browser imports, and history as a spreadsheet-ready CSV.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::bookmarks::{BookmarkNode, MAX_DEPTH};
use crate::ports::store::HistoryVisit;

/// Visits one history export carries, newest first. The store keeps well
/// under this; the bound only keeps a runaway profile from filling memory.
pub const MAX_EXPORT_VISITS: usize = 250_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Bookmarks,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRefusal {
    /// A private profile keeps nothing, so there is nothing to take out.
    Private,
    Unavailable,
}

pub const HISTORY_CSV_HEADER: &str = "date,title,url\r\n";

fn html(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
}

/// Every link and folder, as a Netscape bookmark file. `nodes` holds the whole
/// tree in sibling order; a node whose folder is missing is left out.
pub fn bookmarks_html(nodes: &[BookmarkNode]) -> String {
    let mut children: HashMap<Option<i64>, Vec<&BookmarkNode>> = HashMap::new();
    for node in nodes {
        children.entry(node.parent).or_default().push(node);
    }
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file.\n     \
         It will be read and overwritten.\n     \
         DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n\
         <H1>Bookmarks</H1>\n",
    );
    list(&children, None, 0, &mut out);
    out
}

fn list(
    children: &HashMap<Option<i64>, Vec<&BookmarkNode>>,
    parent: Option<i64>,
    depth: usize,
    out: &mut String,
) {
    let indent = "    ".repeat(depth);
    let _ = writeln!(out, "{indent}<DL><p>");
    for node in children.get(&parent).into_iter().flatten() {
        match &node.url {
            Some(url) => {
                out.push_str(&indent);
                out.push_str("    <DT><A HREF=\"");
                html(url, out);
                out.push_str("\">");
                html(&node.title, out);
                out.push_str("</A>\n");
            }
            None if depth < MAX_DEPTH => {
                out.push_str(&indent);
                out.push_str("    <DT><H3>");
                html(&node.title, out);
                out.push_str("</H3>\n");
                list(children, Some(node.id), depth + 1, out);
            }
            None => {}
        }
    }
    let _ = writeln!(out, "{indent}</DL><p>");
}

/// A field a spreadsheet will read as text. Page titles are chosen by whoever
/// runs the page, so one that opens with a formula character is defused
/// rather than left to run when the file is opened.
fn csv_field(text: &str, out: &mut String) {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
        .collect();
    let defused = text.starts_with(['=', '+', '-', '@', '\t']);
    if defused || text.contains([',', '"', '\t']) {
        out.push('"');
        if defused {
            out.push('\'');
        }
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(&text);
    }
}

/// Appends one `date,title,url` line per visit, the date as UTC ISO 8601.
pub fn push_history_rows(visits: &[HistoryVisit], out: &mut String) {
    for visit in visits {
        out.push_str(&utc(visit.visited_at));
        out.push(',');
        csv_field(&visit.title, out);
        out.push(',');
        csv_field(&visit.url, out);
        out.push_str("\r\n");
    }
}

/// Seconds since the epoch as `2026-10-25T14:03:07Z`.
fn utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from a day count, after Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, parent: Option<i64>, title: &str, url: Option<&str>) -> BookmarkNode {
        BookmarkNode {
            id,
            parent,
            url: url.map(str::to_owned),
            title: title.into(),
            children: 0,
        }
    }

    fn visit(visited_at: i64, title: &str, url: &str) -> HistoryVisit {
        HistoryVisit {
            id: 1,
            url: url.into(),
            title: title.into(),
            visited_at,
        }
    }

    #[test]
    fn bookmarks_nest_in_sibling_order_in_the_netscape_layout() {
        let html = bookmarks_html(&[
            node(1, None, "Reading", None),
            node(2, Some(1), "Rust", Some("https://rust-lang.org/")),
            node(3, None, "Home", Some("https://example.com/")),
            node(4, Some(1), "Deep", None),
            node(5, Some(4), "Leaf", Some("https://leaf.example/")),
        ]);
        assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n"));
        assert!(html
            .contains("<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">"));
        let body = html.split("<H1>Bookmarks</H1>\n").nth(1).unwrap();
        assert_eq!(
            body,
            "<DL><p>\n\
             \x20   <DT><H3>Reading</H3>\n\
             \x20   <DL><p>\n\
             \x20       <DT><A HREF=\"https://rust-lang.org/\">Rust</A>\n\
             \x20       <DT><H3>Deep</H3>\n\
             \x20       <DL><p>\n\
             \x20           <DT><A HREF=\"https://leaf.example/\">Leaf</A>\n\
             \x20       </DL><p>\n\
             \x20   </DL><p>\n\
             \x20   <DT><A HREF=\"https://example.com/\">Home</A>\n\
             </DL><p>\n"
        );
    }

    #[test]
    fn titles_and_addresses_cannot_close_their_tag_or_start_another() {
        let html = bookmarks_html(&[
            node(
                1,
                None,
                "<script>alert(1)</script> & \"co\"",
                Some("https://a.example/?x=1&y=\"><b>"),
            ),
            node(2, None, "Folder </H3><DT><A HREF=\"x\">", None),
        ]);
        assert!(html.contains(
            "<A HREF=\"https://a.example/?x=1&amp;y=&quot;&gt;&lt;b&gt;\">&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;co&quot;</A>"
        ));
        assert!(html.contains("<H3>Folder &lt;/H3&gt;&lt;DT&gt;&lt;A HREF=&quot;x&quot;&gt;</H3>"));
        assert_eq!(html.matches("<A ").count(), 1);
        assert_eq!(html.matches("<H3>").count(), 1);
    }

    #[test]
    fn a_node_without_its_folder_is_left_out_and_an_empty_export_is_valid() {
        let html = bookmarks_html(&[node(2, Some(99), "Orphan", Some("https://o.example/"))]);
        assert!(!html.contains("Orphan"));
        assert!(html.ends_with("<DL><p>\n</DL><p>\n"));
    }

    #[test]
    fn history_rows_carry_utc_date_title_and_address() {
        let mut csv = String::from(HISTORY_CSV_HEADER);
        push_history_rows(
            &[
                visit(0, "Epoch", "https://a.example/"),
                visit(1_793_000_587, "Later", "https://b.example/path?q=1"),
                visit(951_782_400, "Leap day", "https://c.example/"),
                visit(-1, "Before", "https://d.example/"),
            ],
            &mut csv,
        );
        assert_eq!(
            csv,
            "date,title,url\r\n\
             1970-01-01T00:00:00Z,Epoch,https://a.example/\r\n\
             2026-10-26T07:43:07Z,Later,https://b.example/path?q=1\r\n\
             2000-02-29T00:00:00Z,Leap day,https://c.example/\r\n\
             1969-12-31T23:59:59Z,Before,https://d.example/\r\n"
        );
    }

    #[test]
    fn csv_quotes_what_would_split_a_field_and_defuses_formulas() {
        let mut csv = String::new();
        push_history_rows(
            &[
                visit(0, "Hello, \"world\"", "https://a.example/?a=1,2"),
                visit(0, "=HYPERLINK(\"http://evil\")", "https://b.example/"),
                visit(0, "+1 and -2", "https://c.example/"),
                visit(0, "@sum", "https://d.example/"),
                visit(0, "two\nlines\r\n", "https://e.example/"),
                visit(0, "plain - dash", "https://f.example/"),
            ],
            &mut csv,
        );
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "1970-01-01T00:00:00Z,\"Hello, \"\"world\"\"\",\"https://a.example/?a=1,2\""
        );
        assert_eq!(
            lines[1],
            "1970-01-01T00:00:00Z,\"'=HYPERLINK(\"\"http://evil\"\")\",https://b.example/"
        );
        assert_eq!(
            lines[2],
            "1970-01-01T00:00:00Z,\"'+1 and -2\",https://c.example/"
        );
        assert_eq!(
            lines[3],
            "1970-01-01T00:00:00Z,\"'@sum\",https://d.example/"
        );
        // Line breaks inside a title cannot end the record.
        assert_eq!(
            lines[4],
            "1970-01-01T00:00:00Z,two lines  ,https://e.example/"
        );
        assert_eq!(
            lines[5],
            "1970-01-01T00:00:00Z,plain - dash,https://f.example/"
        );
        assert_eq!(lines.len(), 7);
    }
}
