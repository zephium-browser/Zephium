use std::cell::Cell;

use zephium_map::session::Sections;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SectionFlags {
    pub(crate) sections: Sections,
    pub(crate) compare: bool,
}

thread_local! {
    static FLAGS: Cell<SectionFlags> = const { Cell::new(SectionFlags {
        sections: Sections::All, compare: false,
    }) };
}

pub(crate) fn get() -> SectionFlags {
    FLAGS.get()
}

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let (flags, forwarded) = parse(args)?;
    if matches!(args, [help] if help == "--help")
        || matches!(args, [mode, help] if mode == "replay" && help == "--help")
    {
        println!("Session replay options: [--sections all|asked] [--compare-sections]");
    }
    let previous = FLAGS.replace(flags);
    let result = crate::map_bench::run(&forwarded).map_err(|error| error.to_string());
    FLAGS.set(previous);
    result
}

fn parse(args: &[String]) -> Result<(SectionFlags, Vec<String>), String> {
    let mut forwarded = Vec::new();
    let mut sections = None;
    let mut compare = false;
    let mut args_iter = args.iter();
    while let Some(argument) = args_iter.next() {
        match argument.as_str() {
            "--sections" if sections.is_none() => {
                sections = Some(match args_iter.next().map(String::as_str) {
                    Some("all") => Sections::All,
                    Some("asked") => Sections::Asked,
                    _ => return Err("--sections requires all or asked".into()),
                });
            }
            "--compare-sections" if !compare => compare = true,
            "--sections" | "--compare-sections" => {
                return Err("duplicate session replay flag".into())
            }
            _ => forwarded.push(argument.clone()),
        }
    }
    if (sections.is_some() || compare)
        && (args.first().is_none_or(|arg| arg != "replay")
            || forwarded
                .windows(2)
                .any(|pair| pair == ["--engine", "probe"]))
    {
        return Err("section flags require session replay".into());
    }
    Ok((
        SectionFlags {
            sections: sections.unwrap_or_default(),
            compare,
        },
        forwarded,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_bounded_and_specific_to_session_replay() {
        let parse = |args: &str| {
            parse(
                &args
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(parse("replay").unwrap().0.sections, Sections::All);
        assert_eq!(
            parse("replay --sections asked --compare-sections --engine session").unwrap(),
            (
                SectionFlags {
                    sections: Sections::Asked,
                    compare: true
                },
                vec!["replay".to_owned(), "--engine".into(), "session".into()]
            )
        );
        for args in [
            "replay --sections",
            "replay --sections both",
            "replay --sections all --sections all",
            "record --compare-sections",
            "replay --engine probe --sections all",
        ] {
            assert!(parse(args).is_err());
        }
    }
}
