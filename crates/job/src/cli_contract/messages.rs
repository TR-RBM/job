const CATALOGUE: &[(&str, &str)] = &[
    (
        "Native run streams stdout and stderr separately; use --summary for a final diagnostic view.",
        "Native run gibt stdout und stderr getrennt live aus; mit --summary erhältst du die abschließende Diagnoseansicht.",
    ),
    (
        "the attempt changed while waiting; inspect the Job and wait again",
        "der Versuch hat sich während des Wartens geändert; prüfe den Job und warte erneut",
    ),
    (
        "service unavailable while waiting; execution is unknown, wait for the same Job again",
        "Dienst beim Warten nicht erreichbar; Ausführung unbekannt, warte erneut auf denselben Job",
    ),
    (
        "JOB_CLI_COMPAT must be unix or legacy",
        "JOB_CLI_COMPAT muss unix oder legacy sein",
    ),
    (
        "--shell requires an executable name or path",
        "--shell benötigt einen Programmnamen oder Pfad",
    ),
    (
        "--summary and --json are mutually exclusive",
        "--summary und --json schließen sich aus",
    ),
    (
        "--shell and --legacy-shell are mutually exclusive",
        "--shell und --legacy-shell schließen sich aus",
    ),
    ("unexpected service response", "unerwartete Dienstantwort"),
    (
        "usage: job wait [--timeout DURATION] [--summary | --json] ID",
        "Aufruf: job wait [--timeout DAUER] [--summary | --json] ID",
    ),
    (
        "Arguments after -- execute literally. Use --shell SHELL for explicit shell syntax; --legacy-shell enables the previous single-argument Bash rule.",
        "Argumente nach -- werden wörtlich ausgeführt. Verwende --shell SHELL für Shell-Syntax; --legacy-shell aktiviert die bisherige Bash-Regel für ein einzelnes Argument.",
    ),
    (
        "wait is quiet by default; --summary requests a diagnostic summary. JOB_CLI_COMPAT=legacy preserves previous CLI behavior during migration.",
        "wait bleibt standardmäßig still; --summary fordert eine Diagnosezusammenfassung an. JOB_CLI_COMPAT=legacy erhält während der Umstellung das bisherige CLI-Verhalten.",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(en, _)| *en == key)
            .map_or(key, |(_, de)| *de)
            .to_owned()
    } else {
        key.to_owned()
    }
}

pub fn help() -> String {
    [
        "Native run streams stdout and stderr separately; use --summary for a final diagnostic view.",
        "usage: job wait [--timeout DURATION] [--summary | --json] ID",
        "Arguments after -- execute literally. Use --shell SHELL for explicit shell syntax; --legacy-shell enables the previous single-argument Bash rule.",
        "wait is quiet by default; --summary requests a diagnostic summary. JOB_CLI_COMPAT=legacy preserves previous CLI behavior during migration.",
    ].map(message).join("\n")
}
