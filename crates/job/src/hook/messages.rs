const CATALOGUE: &[(&str, &str)] = &[
    ("not an object", "kein Objekt"),
    (
        "hook: standard input is not one JSON object with a command: {error}",
        "hook: die Standardeingabe ist kein einzelnes JSON-Objekt mit einem Befehl: {error}",
    ),
    (
        "job: this line would run detached, where `{prefix}` changes nothing for the caller's shell, so later commands would run in the old directory or environment. Run `{prefix}` as its own call first, then `{rest}`.",
        "job: diese Zeile würde abgelöst laufen, wo `{prefix}` für die Shell des Aufrufers nichts ändert, sodass spätere Befehle im alten Verzeichnis oder in der alten Umgebung liefen. Führe zuerst `{prefix}` als eigenen Aufruf aus, dann `{rest}`.",
    ),
    (
        "job: a `job run` or `job wait` holds its caller until the Job ends; run the rewritten command without waiting for it. Its answer begins with `[job] job N`.",
        "job: ein `job run` oder `job wait` hält seinen Aufrufer fest, bis der Job endet; führe den umgeschriebenen Befehl aus, ohne auf ihn zu warten. Seine Antwort beginnt mit `[job] job N`.",
    ),
    (
        "job: this command would go through the job service because {reason}, but the service does not answer, so it is left to run directly, without a reservation.",
        "job: dieser Befehl ginge durch den job-Dienst, weil {reason}, aber der Dienst antwortet nicht; deshalb bleibt er direkt auszuführen, ohne Reservierung.",
    ),
    (
        "job: routed to the job service because {reason}; run the rewritten command without waiting for it. Its answer begins with `[job] job N`.",
        "job: an den job-Dienst geleitet, weil {reason}; führe den umgeschriebenen Befehl aus, ohne auf ihn zu warten. Seine Antwort beginnt mit `[job] job N`.",
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
