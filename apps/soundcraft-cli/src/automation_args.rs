//! Parse `--automation-read-root` / `--automation-write-root` before the server starts. A malformed
//! flag must not silently remove a root and leave the server unconfined.

use std::path::PathBuf;

#[derive(Default)]
pub struct AutomationArgs {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
}

impl AutomationArgs {
    /// Returns whether this argument is a root flag. Each flag may be given more than once (#874),
    /// in either spelling (`--flag DIR`, `--flag=DIR`): every use adds a folder.
    pub fn consume(&mut self, arg: &str, args: &mut impl Iterator<Item = String>) -> Result<bool, String> {
        let (flag, inline) = arg.split_once('=').map_or((arg, None), |(flag, value)| (flag, Some(value)));
        let roots = match flag {
            "--automation-read-root" => &mut self.read_roots,
            "--automation-write-root" => &mut self.write_roots,
            _ => return Ok(false),
        };
        let value = match inline {
            Some(value) => value.to_owned(),
            None => {
                let value = args.next().ok_or_else(|| format!("{flag} needs a folder"))?;
                if value.starts_with('-') {
                    return Err(format!("{flag} needs a folder; use {flag}=<dir> for a folder starting with '-'"));
                }
                value
            }
        };
        if value.is_empty() {
            return Err(format!("{flag} needs a non-empty folder"));
        }
        roots.push(value.into());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_add_up_and_other_arguments_are_left() {
        let mut roots = AutomationArgs::default();
        let mut args = ["a folder".to_owned(), "song.scraft".to_owned()].into_iter();
        assert!(roots.consume("--automation-read-root", &mut args).unwrap());
        assert!(roots.consume("--automation-write-root=out", &mut args).unwrap());
        assert!(!roots.consume("--demo", &mut args).unwrap());
        assert_eq!(roots.read_roots, [PathBuf::from("a folder")]);
        assert_eq!(roots.write_roots, [PathBuf::from("out")]);
        assert_eq!(args.next().as_deref(), Some("song.scraft"));
        // #874: a flag given again adds a folder, in either spelling.
        assert!(roots.consume("--automation-read-root=other", &mut args).unwrap());
        assert!(roots.consume("--automation-write-root", &mut ["out2".to_owned()].into_iter()).unwrap());
        assert_eq!(roots.read_roots, [PathBuf::from("a folder"), PathBuf::from("other")]);
        assert_eq!(roots.write_roots, [PathBuf::from("out"), PathBuf::from("out2")]);
    }

    #[test]
    fn invalid_operands_leave_roots_unchanged() {
        for flag in ["--automation-read-root", "--automation-write-root"] {
            for operand in [None, Some(""), Some("--version"), Some("--connect"), Some("-folder")] {
                let mut roots = AutomationArgs { read_roots: vec!["read".into()], write_roots: vec!["write".into()] };
                let mut args = operand.map(str::to_owned).into_iter();
                assert!(roots.consume(flag, &mut args).is_err(), "{flag} {operand:?}");
                assert_eq!(roots.read_roots, [PathBuf::from("read")]);
                assert_eq!(roots.write_roots, [PathBuf::from("write")]);
            }
            let mut roots = AutomationArgs::default();
            assert!(roots.consume(&format!("{flag}="), &mut std::iter::empty()).is_err());
        }
    }

    #[test]
    fn explicit_dash_folder_and_relative_dash_folder_are_valid() {
        let mut roots = AutomationArgs::default();
        assert!(roots.consume("--automation-read-root=-folder", &mut std::iter::empty()).unwrap());
        assert!(roots.consume("--automation-write-root", &mut ["./-folder".to_owned()].into_iter()).unwrap());
        assert_eq!(roots.read_roots, [PathBuf::from("-folder")]);
        assert_eq!(roots.write_roots, [PathBuf::from("./-folder")]);
    }
}
