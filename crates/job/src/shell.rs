#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub commands: Vec<Vec<String>>,
    pub background: bool,
}

const LOOP_HEADER_WORDS: [&str; 2] = ["while", "until"];
const SEPARATOR_WORDS: [&str; 12] = [
    "do", "done", "then", "else", "elif", "fi", "if", "case", "esac", "{", "}", "!",
];

fn substitution_end(chars: &[char], open: usize) -> usize {
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            '\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    chars.len()
}

pub fn parse(text: &str) -> Parsed {
    let chars: Vec<char> = text.chars().collect();
    let mut parsed = Parsed {
        commands: Vec::new(),
        background: false,
    };
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut nested: Vec<String> = Vec::new();
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let finish_word = |word: &mut String, in_word: &mut bool, current: &mut Vec<String>| {
        if *in_word {
            current.push(std::mem::take(word));
            *in_word = false;
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                if let Some(&next) = chars.get(i + 1) {
                    if next != '\n' {
                        word.push(next);
                        in_word = true;
                    }
                    i += 1;
                }
            }
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    } else if chars[i] == '`' {
                        let start = i + 1;
                        i += 1;
                        while i < chars.len() && chars[i] != '`' {
                            i += 1;
                        }
                        nested.push(chars[start..i.min(chars.len())].iter().collect());
                        word.push_str("$(…)");
                        i += 1;
                        continue;
                    } else if chars[i] == '$'
                        && chars.get(i + 1) == Some(&'(')
                        && chars.get(i + 2) == Some(&'(')
                    {
                        let end = substitution_end(&chars, i + 1);
                        word.extend(chars[i..(end + 1).min(chars.len())].iter());
                        i = end + 1;
                        continue;
                    } else if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        let end = substitution_end(&chars, i + 1);
                        nested.push(chars[i + 2..end.min(chars.len())].iter().collect());
                        word.push_str("$(…)");
                        i = end;
                        i += 1;
                        continue;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '`' => {
                in_word = true;
                let start = i + 1;
                i += 1;
                while i < chars.len() && chars[i] != '`' {
                    i += 1;
                }
                nested.push(chars[start..i.min(chars.len())].iter().collect());
                word.push_str("$(…)");
            }
            '$' if chars.get(i + 1) == Some(&'(') && chars.get(i + 2) == Some(&'(') => {
                in_word = true;
                let end = substitution_end(&chars, i + 1);
                word.extend(chars[i..(end + 1).min(chars.len())].iter());
                i = end;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                in_word = true;
                let end = substitution_end(&chars, i + 1);
                nested.push(chars[i + 2..end.min(chars.len())].iter().collect());
                word.push_str("$(…)");
                i = end;
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                finish_word(&mut word, &mut in_word, &mut current);
                i += 2;
                let strip_tabs = chars.get(i) == Some(&'-');
                if strip_tabs {
                    i += 1;
                }
                while chars.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
                    i += 1;
                }
                let mut delimiter = String::new();
                while let Some(&d) = chars.get(i) {
                    if d.is_whitespace() || matches!(d, ';' | '|' | '&' | '<' | '>' | '(' | ')') {
                        break;
                    }
                    if d != '\'' && d != '"' && d != '\\' {
                        delimiter.push(d);
                    }
                    i += 1;
                }
                heredocs.push((delimiter, strip_tabs));
                continue;
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            ' ' | '\t' => finish_word(&mut word, &mut in_word, &mut current),
            '|' | ';' | '\n' | '(' | ')' | '&' => {
                finish_word(&mut word, &mut in_word, &mut current);
                let redirect =
                    c == '&' && (chars.get(i + 1) == Some(&'>') || (i > 0 && chars[i - 1] == '>'));
                if redirect {
                    word.push(c);
                    in_word = true;
                } else {
                    if c == '&' && chars.get(i + 1) != Some(&'&') && (i == 0 || chars[i - 1] != '&')
                    {
                        parsed.background = true;
                    }
                    if !current.is_empty() {
                        parsed.commands.push(std::mem::take(&mut current));
                    }
                }
                if c == '\n' && !heredocs.is_empty() {
                    i += 1;
                    for (delimiter, strip_tabs) in heredocs.drain(..) {
                        while i < chars.len() {
                            let end = (i..chars.len())
                                .find(|&j| chars[j] == '\n')
                                .unwrap_or(chars.len());
                            let line: String = chars[i..end].iter().collect();
                            i = end + 1;
                            let line = if strip_tabs {
                                line.trim_start_matches('\t')
                            } else {
                                line.as_str()
                            };
                            if line == delimiter {
                                break;
                            }
                        }
                    }
                    continue;
                }
            }
            _ => {
                word.push(c);
                in_word = true;
            }
        }
        i += 1;
    }
    finish_word(&mut word, &mut in_word, &mut current);
    if !current.is_empty() {
        parsed.commands.push(current);
    }
    let mut simple = Vec::new();
    for command in parsed.commands.drain(..) {
        let mut words: Vec<String> = Vec::new();
        let mut skip_next = false;
        for w in command {
            if skip_next {
                skip_next = false;
                continue;
            }
            if is_redirection(&w) {
                skip_next = redirection_needs_target(&w);
                continue;
            }
            words.push(w);
        }
        while let Some(first) = words.first() {
            if first == "for" || first == "select" {
                words.clear();
            } else if SEPARATOR_WORDS.contains(&first.as_str())
                || LOOP_HEADER_WORDS.contains(&first.as_str())
                || is_assignment(first)
            {
                words.remove(0);
            } else {
                break;
            }
        }
        if !words.is_empty() {
            simple.push(words);
        }
    }
    parsed.commands = simple;
    for inner in nested {
        let inner = parse(&inner);
        parsed.background |= inner.background;
        parsed.commands.extend(inner.commands);
    }
    parsed
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn is_redirection(word: &str) -> bool {
    let rest = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
    rest.starts_with('>') || rest.starts_with('<')
}

fn redirection_needs_target(word: &str) -> bool {
    let rest = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
    let operator_only = rest.trim_start_matches(['>', '<', '&', '|']).is_empty();
    operator_only && !rest.ends_with('&') && !word.contains(">&")
}

pub fn program(words: &[String]) -> String {
    let first = words.first().map(String::as_str).unwrap_or("");
    std::path::Path::new(first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first)
        .to_string()
}

const WRAPPERS: [&str; 14] = [
    "time", "nice", "nohup", "setsid", "stdbuf", "ionice", "chpst", "exec", "command", "builtin",
    "sudo", "doas", "env", "xargs",
];

fn options_with_values(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "time" => &["-f", "-o"],
        "nice" => &["-n"],
        "ionice" => &["-c", "-n", "-p", "-P", "-u"],
        "stdbuf" => &["-i", "-o", "-e"],
        "chpst" => &[
            "-u", "-U", "-b", "-e", "-/", "-C", "-n", "-l", "-L", "-m", "-d", "-o", "-p", "-f",
            "-c", "-r", "-t",
        ],
        "exec" => &["-a"],
        "sudo" => &[
            "-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-T", "-U", "-R",
        ],
        "doas" => &["-u", "-C"],
        "env" => &["-u", "-C", "-S"],
        "xargs" => &["-n", "-I", "-P", "-L", "-d", "-s", "-a", "-E"],
        "timeout" => &["-s", "-k"],
        "flock" => &["-w", "-E"],
        _ => &[],
    }
}

pub fn unwrap_command(words: &[String]) -> Vec<String> {
    let mut words = words.to_vec();
    loop {
        let name = program(&words);
        let takes_argument = matches!(name.as_str(), "timeout" | "flock");
        if !WRAPPERS.contains(&name.as_str()) && !takes_argument {
            return words;
        }
        if name == "flock" && words.iter().any(|w| w == "-c") {
            let inner = words
                .iter()
                .skip_while(|w| *w != "-c")
                .nth(1)
                .cloned()
                .unwrap_or_default();
            return vec!["bash".to_string(), "-c".to_string(), inner];
        }
        words.remove(0);
        let with_values = options_with_values(&name);
        while let Some(first) = words.first() {
            if with_values.contains(&first.as_str()) {
                words.drain(..2.min(words.len()));
            } else if first.starts_with('-') || first.contains('=') {
                words.remove(0);
            } else {
                break;
            }
        }
        if takes_argument && !words.is_empty() {
            words.remove(0);
        }
    }
}

pub fn simple_commands(text: &str) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for words in parse(text).commands {
        let words = unwrap_command(&words);
        let shell_line = matches!(program(&words).as_str(), "bash" | "sh")
            .then(|| words.iter().skip_while(|w| *w != "-c").nth(1).cloned())
            .flatten();
        match shell_line {
            Some(inner) => found.extend(simple_commands(&inner)),
            None => found.push(words),
        }
    }
    found
}

const STATE_COMMANDS: [&str; 12] = [
    "cd", "pushd", "popd", "export", "unset", "set", "source", ".", "alias", "unalias", "umask",
    "shopt",
];

fn is_state_segment(segment: &str) -> bool {
    let parsed = parse(segment);
    match parsed.commands.first() {
        Some(words) => STATE_COMMANDS.contains(&words[0].as_str()),
        None => !segment.trim().is_empty() && segment.contains('='),
    }
}

pub fn split_state_prefix(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut prefix_end = 0;
    let mut segment_start = 0;
    let mut depth = 0i32;
    let (mut single, mut double, mut backtick) = (false, false, false);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if single {
            single = c != b'\'';
        } else if c == b'\\' {
            i += 1;
        } else if double {
            double = c != b'"';
        } else if backtick {
            backtick = c != b'`';
        } else {
            match c {
                b'\'' => single = true,
                b'"' => double = true,
                b'`' => backtick = true,
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ if depth > 0 => {}
                b'#' if i == 0 || bytes[i - 1].is_ascii_whitespace() => break,
                b'<' if bytes.get(i + 1) == Some(&b'<') => break,
                b'|' => break,
                b'&' if bytes.get(i + 1) == Some(&b'&') => {
                    if !is_state_segment(&text[segment_start..i]) {
                        break;
                    }
                    i += 2;
                    prefix_end = i;
                    segment_start = i;
                    continue;
                }
                b'&' => break,
                b';' | b'\n' => {
                    if !is_state_segment(&text[segment_start..i]) {
                        break;
                    }
                    prefix_end = i + 1;
                    segment_start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    (&text[..prefix_end], text[prefix_end..].trim_start())
}

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programs(text: &str) -> Vec<String> {
        parse(text)
            .commands
            .into_iter()
            .map(|c| c[0].clone())
            .collect()
    }

    #[test]
    fn pipes_and_lists_split_into_simple_commands() {
        assert_eq!(
            programs("cargo test 2>&1 | tail -5 && echo ok; ls"),
            vec!["cargo", "tail", "echo", "ls"]
        );
    }

    #[test]
    fn quotes_keep_operators_inside_one_word() {
        let parsed = parse("grep 'a|b; c' file && echo \"x && y\"");
        assert_eq!(parsed.commands[0], vec!["grep", "a|b; c", "file"]);
        assert_eq!(parsed.commands[1], vec!["echo", "x && y"]);
    }

    #[test]
    fn assignments_and_redirections_are_not_the_program() {
        assert_eq!(
            programs("RUST_LOG=debug cargo build > out.txt 2>&1"),
            vec!["cargo"]
        );
        assert_eq!(parse("cat < input.txt").commands[0], vec!["cat"]);
    }

    #[test]
    fn command_substitutions_are_commands_too() {
        assert_eq!(
            programs("echo $(cargo metadata --format-version 1)"),
            vec!["echo", "cargo"]
        );
        assert_eq!(programs("echo \"`make`\""), vec!["echo", "make"]);
    }

    #[test]
    fn loops_give_their_body_and_a_while_condition_is_a_command() {
        assert_eq!(
            parse("for r in one two; do git -C $r status; done").commands,
            vec![vec!["git", "-C", "$r", "status"]]
        );
        let parsed = parse("git branch | while read b; do git ls-tree $b; done");
        let programs: Vec<&str> = parsed.commands.iter().map(|c| c[0].as_str()).collect();
        assert_eq!(programs, vec!["git", "read", "git"]);
        let parsed = parse("until test -f done.txt; do sleep 5; done");
        let programs: Vec<&str> = parsed.commands.iter().map(|c| c[0].as_str()).collect();
        assert_eq!(programs, vec!["test", "sleep"]);
    }

    #[test]
    fn a_trailing_ampersand_is_background_and_a_redirection_is_not() {
        assert!(parse("sleep 10 &").background);
        assert!(!parse("cargo test 2>&1 | tail").background);
        assert!(!parse("make &> log.txt").background);
        assert!(!parse("a && b").background);
    }

    #[test]
    fn heredoc_bodies_are_not_commands() {
        let text = "cat > notes.md <<'EOF'\nfn main() {}\n- a list item\nEOF\ngit status";
        assert_eq!(programs(text), vec!["cat", "git"]);
        let text = "python3 - <<-END\n\tprint(1)\n\tEND\nls";
        assert_eq!(programs(text), vec!["python3", "ls"]);
        assert_eq!(programs("grep x <<< \"$text\""), vec!["grep"]);
    }

    #[test]
    fn simple_commands_look_through_cd_wrappers_and_shell_lines() {
        let found = simple_commands("cd /repo && time bash -c 'nice -n 5 cargo test -p x' | tail");
        let programs: Vec<String> = found.iter().map(|c| program(c)).collect();
        assert_eq!(programs, vec!["cd", "cargo", "tail"]);
        assert_eq!(found[1], vec!["cargo", "test", "-p", "x"]);
    }

    #[test]
    fn arithmetic_expansion_is_part_of_a_word_not_a_command() {
        assert_eq!(
            programs("N=$((10#$N+1)); echo \"$((N * 2))\""),
            vec!["echo"]
        );
        assert_eq!(programs("echo $(( $(wc -l < f) + 1 ))"), vec!["echo"]);
    }

    #[test]
    fn a_substitution_leaves_a_marker_in_its_word() {
        let parsed = parse("rm $(ls) \"`pwd`/x\"");
        assert_eq!(parsed.commands[0], vec!["rm", "$(…)", "$(…)/x"]);
    }

    #[test]
    fn leading_shell_state_commands_are_split_off() {
        assert_eq!(
            split_state_prefix("cd /home/agent/work/lead/x && python3 edit.py"),
            ("cd /home/agent/work/lead/x &&", "python3 edit.py")
        );
        assert_eq!(
            split_state_prefix("export RUST_LOG=debug; cd 'a b' && cargo test"),
            ("export RUST_LOG=debug; cd 'a b' &&", "cargo test")
        );
        assert_eq!(
            split_state_prefix("X=1\ncargo build"),
            ("X=1\n", "cargo build")
        );
        assert_eq!(
            split_state_prefix("cargo build && cd x"),
            ("", "cargo build && cd x")
        );
        assert_eq!(split_state_prefix("cd x | cat"), ("", "cd x | cat"));
        assert_eq!(
            split_state_prefix("cd x || exit 1; make"),
            ("", "cd x || exit 1; make")
        );
        assert_eq!(
            split_state_prefix("cd $(git rev-parse --show-toplevel) && make"),
            ("cd $(git rev-parse --show-toplevel) &&", "make")
        );
        assert_eq!(
            split_state_prefix("echo 'cd x && y' && make"),
            ("", "echo 'cd x && y' && make")
        );
        assert_eq!(
            split_state_prefix("cd x && cat <<EOF\nhi\nEOF"),
            ("cd x &&", "cat <<EOF\nhi\nEOF")
        );
    }

    #[test]
    fn comments_are_ignored() {
        assert_eq!(programs("ls # cargo build"), vec!["ls"]);
    }

    #[test]
    fn quoting_survives_single_quotes_inside() {
        assert_eq!(quote("echo 'hi'"), "'echo '\\''hi'\\'''");
        assert_eq!(
            parse(&format!("bash -c {}", quote("echo 'hi' | tail"))).commands[0][2],
            "echo 'hi' | tail"
        );
    }

    fn unwrapped(text: &str) -> Vec<String> {
        simple_commands(text)
            .into_iter()
            .map(|w| program(&w))
            .collect()
    }

    #[test]
    fn each_wrapper_skips_only_its_own_options_that_take_a_value() {
        assert_eq!(unwrapped("sudo -n sv restart job"), vec!["sv"]);
        assert_eq!(unwrapped("sudo -n cp a job.new"), vec!["cp"]);
        assert_eq!(unwrapped("sudo -u agent -n cargo test"), vec!["cargo"]);
        assert_eq!(unwrapped("doas -u root sv status job"), vec!["sv"]);
        assert_eq!(unwrapped("nice -n 10 cargo build"), vec!["cargo"]);
        assert_eq!(unwrapped("xargs -n 1 echo"), vec!["echo"]);
        assert_eq!(unwrapped("flock -n /tmp/lock cargo test"), vec!["cargo"]);
        assert_eq!(unwrapped("timeout -s KILL 10 cargo test"), vec!["cargo"]);
    }
}
