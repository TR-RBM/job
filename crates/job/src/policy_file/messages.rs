const CATALOGUE: &[(&str, &str)] = &[
    (
        "not a valid policy file: {error}",
        "keine gültige Richtliniendatei: {error}",
    ),
    (
        "unknown rule {rule}; the rules are {rules}",
        "unbekannte Regel {rule}; die Regeln sind {rules}",
    ),
    (
        "rule {rule} needs a text for forbids and for instead",
        "Regel {rule} braucht einen Text für forbids und für instead",
    ),
    (
        "{field} must be an absolute path",
        "{field} muss ein absoluter Pfad sein",
    ),
    (
        "rule other-worktree needs work_root or repos_root",
        "Regel other-worktree braucht work_root oder repos_root",
    ),
    (
        "rule rm-protected needs protected_paths",
        "Regel rm-protected braucht protected_paths",
    ),
    (
        "cannot read the policy file: {error}",
        "die Richtliniendatei ist nicht lesbar: {error}",
    ),
    (
        "no policy file at {places}; the command policy forbids nothing",
        "keine Richtliniendatei unter {places}; die Befehlsrichtlinie verbietet nichts",
    ),
    (
        "{path} is valid and names no rule; the command policy forbids nothing",
        "{path} ist gültig und nennt keine Regel; die Befehlsrichtlinie verbietet nichts",
    ),
    (
        "{path} is valid; rules in force: {rules}",
        "{path} ist gültig; geltende Regeln: {rules}",
    ),
    (
        "job: the command policy cannot be read, so every command is refused until the file is corrected or removed: {error}",
        "job: die Befehlsrichtlinie ist nicht lesbar, deshalb wird jeder Befehl abgelehnt, bis du die Datei korrigierst oder entfernst: {error}",
    ),
    (
        "correct the file or remove it; job policy --show validates it",
        "korrigiere die Datei oder entferne sie; job policy --show prüft sie",
    ),
    (
        "job: the hook failed while checking this command, and the command matches a known-dangerous pattern, so it is refused; the rules are in {path}",
        "job: der Hook ist beim Prüfen dieses Befehls gescheitert, und der Befehl passt auf ein bekannt gefährliches Muster, deshalb wird er abgelehnt; die Regeln stehen in {path}",
    ),
];

pub fn message(english: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let template = if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
    } else {
        english
    };
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}
