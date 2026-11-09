//! Heuristic detection of dangerous steps, used to demand explicit confirmation.
//!
//! This is a speed bump for the user's own recordings, **not** a security boundary: a
//! determined author can always disguise a command. It works on parsed words rather than
//! substrings, so `echo "rm -rf"` and `confirm -r` are not flagged.

/// Whether a step contains a command that deletes data or rewrites history.
pub fn is_destructive(step: &str) -> bool {
    let words =
        shlex::split(step).unwrap_or_else(|| step.split_whitespace().map(str::to_owned).collect());
    words
        .split(|w| matches!(w.as_str(), "&&" | "||" | ";" | "|"))
        .any(is_destructive_command)
}

fn is_destructive_command(words: &[String]) -> bool {
    let Some(program) = words.first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let args = &words[1..];
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let has_short = |letters: &[char]| {
        args.iter().any(|a| {
            a.starts_with('-') && !a.starts_with("--") && a.chars().any(|c| letters.contains(&c))
        })
    };

    if args
        .iter()
        .any(|a| a == "--force" || a.starts_with("--force-"))
        && program != "git"
    {
        return true;
    }
    match program {
        "sudo" | "doas" | "shred" | "wipefs" | "mkfs" => true,
        p if p.starts_with("mkfs.") => true,
        "rm" => has_short(&['r', 'R', 'f']) || has("--recursive") || has("--force"),
        "dd" => args.iter().any(|a| a.starts_with("of=")),
        "git" => git_is_destructive(args),
        _ => sql_drop(program, words),
    }
}

fn git_is_destructive(args: &[String]) -> bool {
    let sub = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    let flag = |f: &str| args.iter().any(|a| a == f);
    let short = |c: char| {
        args.iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(c))
    };
    match sub {
        Some("push") => {
            flag("--force")
                || flag("-f")
                || flag("--delete")
                || args.iter().any(|a| a.starts_with("--force-"))
        }
        Some("reset") => flag("--hard"),
        Some("clean") => short('f'),
        Some("branch") => flag("-D"),
        Some("checkout" | "restore") => flag("--force") || flag("-f") || flag("."),
        _ => false,
    }
}

const SQL_CLIENTS: &[&str] = &[
    "psql",
    "mysql",
    "mariadb",
    "sqlite3",
    "sqlcmd",
    "clickhouse-client",
];

/// `DROP TABLE|DATABASE|SCHEMA`: as separate words anywhere, or inside the single quoted
/// argument handed to a database client (`psql -c 'DROP TABLE users'`).
fn sql_drop(program: &str, words: &[String]) -> bool {
    let is_drop = |text: &str| {
        let mut it = text.split_whitespace();
        it.by_ref().any(|w| w.eq_ignore_ascii_case("drop"))
            && it.next().is_some_and(|kind| {
                ["table", "database", "schema"]
                    .iter()
                    .any(|k| kind.eq_ignore_ascii_case(k))
            })
    };
    words
        .windows(2)
        .any(|w| w[0].eq_ignore_ascii_case("drop") && is_drop(&format!("{} {}", w[0], w[1])))
        || (SQL_CLIENTS.contains(&program) && words.iter().any(|w| is_drop(w)))
}

#[cfg(test)]
mod tests {
    use super::is_destructive;

    #[test]
    fn flags_commands_that_destroy_data() {
        for step in [
            "rm -rf ./build",
            "rm -fr x",
            "rm -r dir",
            "rm --recursive dir",
            "cd x && rm -rf y",
            "sudo apt remove foo",
            "git push --force origin main",
            "git push -f",
            "git push origin --force-with-lease",
            "git reset --hard HEAD~3",
            "git clean -fd",
            "git branch -D feature",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sdb1",
            "psql -c 'DROP TABLE users'",
            "docker rm --force web",
            "/bin/rm -rf /tmp/x",
        ] {
            assert!(is_destructive(step), "{step}");
        }
    }

    #[test]
    fn leaves_ordinary_commands_alone() {
        for step in [
            "echo \"rm -rf\"",
            "confirm -r",
            "git push origin main",
            "git push -u origin main",
            "git reset --soft HEAD~1",
            "git branch -d merged",
            "rm file.txt",
            "ls -rf",
            "pnpm add zustand",
            "dd if=/dev/zero bs=1 count=1",
        ] {
            assert!(!is_destructive(step), "{step}");
        }
    }

    #[test]
    fn unbalanced_quotes_fall_back_to_plain_words() {
        assert!(is_destructive("cat <<EOF\nit's\nEOF\n; rm -rf /tmp/x"));
    }
}
