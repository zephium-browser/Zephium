mod corpus;
mod engine;
mod platform;
mod probe;
mod report;
mod run;
mod score;
mod session;
mod store;

use std::fmt;
use std::path::{Path, PathBuf};

use engine::{Engine, EngineKind};

use corpus::{valid_id, Corpus, CorpusQuery, MAX_CORPUS_FILES, MAX_QUERIES};

const MAX_URL_BYTES: usize = zephium_public_fetch::MAX_LOCATION_BYTES;
const USAGE: &str = "\
cargo xtask map-bench record --corpus <file>... --out <dir> [--only <id>...] [--limit N] [--resume] [--engine probe|session]
cargo xtask map-bench replay --corpus <file>... --from <dir> [--only <id>...] [--engine probe|session]
cargo xtask map-bench report --from <dir> [--markdown <file>]";

type Result<T> = std::result::Result<T, BenchError>;

#[derive(Debug)]
pub(crate) struct BenchError {
    class: &'static str,
    io_kind: Option<std::io::ErrorKind>,
    corpus: Option<usize>,
    entry: Option<usize>,
    line: Option<usize>,
    query: Option<String>,
}

impl BenchError {
    fn new(class: &'static str) -> Self {
        Self {
            class,
            io_kind: None,
            corpus: None,
            entry: None,
            line: None,
            query: None,
        }
    }

    fn io(class: &'static str, error: &std::io::Error) -> Self {
        Self {
            io_kind: Some(error.kind()),
            ..Self::new(class)
        }
    }

    fn at(self, corpus: usize) -> Self {
        Self {
            corpus: Some(corpus),
            ..self
        }
    }

    fn entry(self, entry: usize) -> Self {
        Self {
            entry: Some(entry),
            ..self
        }
    }

    fn line(self, line: usize) -> Self {
        Self {
            line: Some(line),
            ..self
        }
    }

    fn for_query(self, id: &str) -> Self {
        Self {
            query: valid_id(id).then(|| id.to_owned()),
            ..self
        }
    }
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.class)?;
        if let Some(kind) = self.io_kind {
            write!(formatter, " ({kind:?})")?;
        }
        if let Some(corpus) = self.corpus {
            write!(formatter, " in corpus file {corpus}")?;
        }
        if let Some(entry) = self.entry {
            write!(formatter, " query {entry}")?;
        }
        if let Some(line) = self.line {
            write!(formatter, " at line {line}")?;
        }
        if let Some(id) = &self.query {
            write!(formatter, " for query {id}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Record,
    Replay,
    Report,
}

#[derive(Debug)]
struct Options {
    mode: Mode,
    directory: PathBuf,
    corpora: Vec<PathBuf>,
    only: Vec<String>,
    limit: usize,
    resume: bool,
    engine: EngineKind,
    markdown: Option<PathBuf>,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self> {
        let (mode, arguments) = match arguments.split_first() {
            Some((mode, arguments)) => (
                match mode.as_str() {
                    "record" => Mode::Record,
                    "replay" => Mode::Replay,
                    "report" => Mode::Report,
                    _ => return Err(BenchError::new("unknown_mode")),
                },
                arguments,
            ),
            None => return Err(BenchError::new("missing_mode")),
        };
        let mut arguments = arguments.iter().peekable();
        let mut directory = None;
        let mut corpora = Vec::new();
        let mut only = Vec::new();
        let mut limit = None;
        let mut resume = false;
        let mut engine = None;
        let mut markdown = None;
        while let Some(flag) = arguments.next() {
            let has_value = |next: &&String| !next.starts_with("--");
            let mut value = || {
                arguments
                    .next_if(has_value)
                    .ok_or(BenchError::new("missing_option_value"))
            };
            match (mode, flag.as_str()) {
                (Mode::Record, "--out") | (Mode::Replay | Mode::Report, "--from")
                    if directory.is_none() =>
                {
                    directory = Some(PathBuf::from(value()?));
                }
                (Mode::Record | Mode::Replay, "--corpus") => {
                    corpora.push(PathBuf::from(value()?));
                    while let Some(path) = arguments.next_if(has_value) {
                        if corpora.len() >= MAX_CORPUS_FILES {
                            return Err(BenchError::new("corpus_file_count"));
                        }
                        corpora.push(PathBuf::from(path));
                    }
                    if corpora.len() > MAX_CORPUS_FILES {
                        return Err(BenchError::new("corpus_file_count"));
                    }
                }
                (Mode::Record | Mode::Replay, "--only") => {
                    let mut id = Some(value()?);
                    while let Some(next) = id {
                        if only.len() >= MAX_QUERIES || !valid_id(next) {
                            return Err(BenchError::new("invalid_only_id_or_count"));
                        }
                        only.push(next.clone());
                        id = arguments.next_if(has_value);
                    }
                }
                (Mode::Record | Mode::Replay, "--engine") if engine.is_none() => {
                    engine = Some(match value()?.as_str() {
                        "probe" => EngineKind::Probe,
                        "session" => EngineKind::Session,
                        _ => return Err(BenchError::new("unknown_engine")),
                    });
                }
                (Mode::Record, "--resume") if !resume => resume = true,
                (Mode::Record, "--limit") if limit.is_none() => {
                    limit = Some(
                        value()?
                            .parse()
                            .ok()
                            .filter(|n| (1..=MAX_QUERIES).contains(n))
                            .ok_or(BenchError::new("invalid_limit"))?,
                    );
                }
                (Mode::Report, "--markdown") if markdown.is_none() => {
                    markdown = Some(PathBuf::from(value()?))
                }
                _ => return Err(BenchError::new("unexpected_or_duplicate_option")),
            }
        }
        let directory = directory.ok_or(BenchError::new("missing_run_directory"))?;
        if mode != Mode::Report && corpora.is_empty() {
            return Err(BenchError::new("missing_corpus"));
        }
        Ok(Self {
            mode,
            directory,
            corpora,
            only,
            limit: limit.unwrap_or(MAX_QUERIES),
            resume,
            engine: engine.unwrap_or(EngineKind::Session),
            markdown,
        })
    }

    fn select<'a>(&self, corpus: &'a Corpus) -> Result<Vec<&'a CorpusQuery>> {
        for id in &self.only {
            if !corpus.queries.iter().any(|query| query.label.id == *id) {
                return Err(BenchError::new("unknown_query_id").for_query(id));
            }
        }
        Ok(corpus
            .queries
            .iter()
            .filter(|query| self.only.is_empty() || self.only.contains(&query.label.id))
            .collect())
    }
}

pub(crate) fn run(arguments: &[String]) -> Result<()> {
    if arguments == ["--help"]
        || arguments.len() == 2
            && arguments[1] == "--help"
            && matches!(arguments[0].as_str(), "record" | "replay" | "report")
    {
        println!("{USAGE}");
        return Ok(());
    }
    let options = Options::parse(arguments).inspect_err(|_| eprintln!("{USAGE}"))?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = store::external_path(&options.directory, &repository)?;
    if options.mode == Mode::Report {
        let markdown = options
            .markdown
            .as_deref()
            .map(|path| store::external_path(path, &repository))
            .transpose()?;
        return report::write(&root, markdown.as_deref());
    }
    let corpus = Corpus::load(&options.corpora)?;
    let selected = options.select(&corpus)?;
    let engine: &mut dyn Engine = match options.engine {
        EngineKind::Probe => &mut probe::ProbeEngine,
        EngineKind::Session => &mut session::SessionEngine,
    };
    match options.mode {
        Mode::Record => run::record(&repository, &root, &corpus, selected, &options, engine),
        Mode::Replay => run::replay(&root, &corpus, selected, engine, options.engine),
        Mode::Report => Err(BenchError::new("invalid_mode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_mode_specific_and_bound() {
        let parse = |text: &str| {
            Options::parse(
                &text
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        };
        let options =
            parse("record --corpus a b --corpus c --out r --only a b --only c --limit 2 --resume")
                .unwrap();
        assert_eq!(options.corpora.len(), 3);
        assert_eq!(options.only, ["a", "b", "c"]);
        assert_eq!(options.limit, 2);
        assert!(options.resume);
        assert_eq!(options.engine, EngineKind::Session);
        assert_eq!(
            parse("replay --corpus a --from dir --engine probe")
                .unwrap()
                .engine,
            EngineKind::Probe
        );
        assert!(parse("replay --corpus a --from dir --only a").is_ok());
        assert!(parse("report --from dir --markdown path").is_ok());

        for invalid in [
            "",
            "other",
            "record --out /tmp",
            "record --corpus --out /tmp",
            "record --corpus a --out r --limit 0",
            "record --corpus a --out r --out s",
            "replay --corpus a --from r --resume",
            "report --from r --corpus a",
            "report --from r --markdown",
            "report --from r --engine session",
            "replay --corpus a --from r --engine invalid",
            "replay --corpus a --from r --engine session --engine probe",
            "replay --corpus a --from r --only ../r",
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
        assert!(parse(&format!(
            "record --corpus a --out r --limit {}",
            MAX_QUERIES + 1
        ))
        .is_err());
        assert!(parse(&format!(
            "record --corpus {}--out r",
            "a ".repeat(MAX_CORPUS_FILES + 1)
        ))
        .is_err());
        assert!(parse(&format!(
            "replay --corpus a --from r --only {}",
            "q ".repeat(MAX_QUERIES + 1)
        ))
        .is_err());
    }
}
