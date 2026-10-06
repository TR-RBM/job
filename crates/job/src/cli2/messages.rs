const CATALOGUE: &[(&str, &str)] = &[
    (
        "the running service is older than this command; restart it from the same release",
        "der laufende Dienst ist älter als dieser Befehl; starte ihn aus derselben Version neu",
    ),
    (
        "`{word}` is not a Job state; write one or more of held, queued, starting, running, suspended, stopping, succeeded, failed, cancelled, lost, separated by commas",
        "`{word}` ist kein Job-Zustand; schreibe einen oder mehrere von held, queued, starting, running, suspended, stopping, succeeded, failed, cancelled, lost, durch Kommas getrennt",
    ),
    (
        "--all and --state are mutually exclusive",
        "--all und --state schließen sich aus",
    ),
    (
        "--limit: `{word}` is not a number from 1 through 10000",
        "--limit: `{word}` ist keine Zahl von 1 bis 10000",
    ),
    ("no Jobs match", "kein Job passt"),
    ("ID", "ID"),
    ("STATE", "ZUSTAND"),
    ("QUEUE", "QUEUE"),
    ("SESSION", "SITZUNG"),
    ("EXIT", "ENDE"),
    ("LABELS", "LABELS"),
    ("COMMAND", "BEFEHL"),
    (
        "more Jobs match than were listed; raise --limit or narrow --state, --queue or --label",
        "es passen mehr Jobs, als aufgelistet wurden; erhöhe --limit oder grenze mit --state, --queue oder --label ein",
    ),
    ("there is no Queue `{word}`", "es gibt keine Queue `{word}`"),
    ("there is no Job {id}", "es gibt keinen Job {id}"),
    (
        "Job {id} is {state}; only held and queued Jobs can be moved",
        "Job {id} ist {state}; nur gehaltene und wartende Jobs lassen sich verschieben",
    ),
    (
        "the saved environment of the Job is unavailable; it was not moved",
        "die gespeicherte Umgebung des Jobs fehlt; er wurde nicht verschoben",
    ),
    (
        "Job {id} is in Queue {queue}",
        "Job {id} ist in Queue {queue}",
    ),
    (
        "--format tsv and --json are mutually exclusive",
        "--format tsv und --json schließen sich aus",
    ),
    (
        "`{word}` is not a label; write KEY=VALUE with a key of 1 to 63 characters from a-z, 0-9, dot, underscore and dash and a value of at most 255 bytes without control characters",
        "`{word}` ist kein Label; schreibe KEY=VALUE mit einem Schlüssel aus 1 bis 63 Zeichen aus a-z, 0-9, Punkt, Unterstrich und Bindestrich und einem Wert von höchstens 255 Bytes ohne Steuerzeichen",
    ),
    (
        "label `{word}` is given twice",
        "das Label `{word}` ist doppelt angegeben",
    ),
    (
        "an object carries at most 32 labels",
        "ein Objekt trägt höchstens 32 Labels",
    ),
    ("invalid label `{word}`", "ungültiges Label `{word}`"),
    (
        "labels are stored as a map of text",
        "Labels werden als Zuordnung von Text gespeichert",
    ),
    ("labels", "Labels"),
    (
        "--on-interrupt: `{word}` is not a choice; write forward, detach or cancel",
        "--on-interrupt: `{word}` ist keine Auswahl; schreibe forward, detach oder cancel",
    ),
    (
        "the terminal hung up; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}",
        "das Terminal hat aufgelegt; Job {id} läuft weiter; warte mit job wait {id} auf ihn, brich ihn mit job cancel {id} ab",
    ),
    (
        "interrupted; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}",
        "unterbrochen; Job {id} läuft weiter; warte mit job wait {id} auf ihn, brich ihn mit job cancel {id} ab",
    ),
    (
        "the output was closed; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}",
        "die Ausgabe wurde geschlossen; Job {id} läuft weiter; warte mit job wait {id} auf ihn, brich ihn mit job cancel {id} ab",
    ),
    (
        "{detail} forwarded to Job {id}; interrupt again to leave it running and return",
        "{detail} an Job {id} weitergegeben; unterbrich noch einmal, um ihn laufen zu lassen und zurückzukehren",
    ),
    (
        "Job {id} did not receive the signal: {detail}; interrupt again to leave it and return",
        "Job {id} hat das Signal nicht erhalten: {detail}; unterbrich noch einmal, um ihn zu verlassen und zurückzukehren",
    ),
    (
        "Job {id} was not cancelled: {detail}; interrupt again to leave it and return",
        "Job {id} wurde nicht abgebrochen: {detail}; unterbrich noch einmal, um ihn zu verlassen und zurückzukehren",
    ),
    (
        "cancelling Job {id}; interrupt again to return without waiting",
        "Job {id} wird abgebrochen; unterbrich noch einmal, um ohne Warten zurückzukehren",
    ),
    (
        "--on-interrupt goes with run in pipe mode; with --pty the terminal carries Ctrl-C to the Job",
        "--on-interrupt gehört zu run im Pipe-Modus; mit --pty trägt das Terminal Strg-C zum Job",
    ),
    (
        "ancestor {path} sets a lower {option}: {value}; the lower one applies",
        "der Vorfahr {path} setzt ein niedrigeres {option}: {value}; der niedrigere Wert gilt",
    ),
    (
        "unlimited here, but ancestor {path} still bounds this subtree: {option} {value}",
        "hier unbegrenzt, aber der Vorfahr {path} begrenzt diesen Teilbaum weiterhin: {option} {value}",
    ),
    (
        "each Job may have {value}, but all Jobs below {path} together are bounded by {option} {bound}",
        "jeder Job darf {value} haben, aber alle Jobs unter {path} zusammen sind durch {option} {bound} begrenzt",
    ),
    (
        "a Job with this request is never admitted: the admission budget {option} at {path} is {bound}",
        "ein Job mit dieser Anforderung wird nie zugelassen: das Zulassungsbudget {option} bei {path} ist {bound}",
    ),
    (
        "this host cannot apply {file}: the service has no delegated cgroup controller for it, so Jobs that need it are refused",
        "dieser Host kann {file} nicht anwenden: der Dienst hat dafür keinen delegierten cgroup-Controller, daher werden Jobs abgelehnt, die es brauchen",
    ),
    ("not set", "nicht gesetzt"),
    (
        "no setting is configured here or inherited",
        "hier ist keine Einstellung gesetzt oder geerbt",
    ),
    (
        "{option}: configured {configured}; effective {effective}{source}",
        "{option}: gesetzt {configured}; wirksam {effective}{source}",
    ),
    ("; supplied by {path}", "; geliefert von {path}"),
    (
        "there is no Queue or Group `{word}`",
        "es gibt keine Queue oder Gruppe `{word}`",
    ),
    (
        "the service sets no file system quota; --write-budget counts the bytes a Job writes and stops the Job when it passes the budget",
        "der Dienst setzt kein Dateisystemkontingent; --write-budget zählt die Bytes, die ein Job schreibt, und hält den Job an, wenn er das Budget überschreitet",
    ),
    (
        "--stdin goes with pipe mode; with --pty the terminal is the input of the Job",
        "--stdin gehört zum Pipe-Modus; mit --pty ist das Terminal die Eingabe des Jobs",
    ),
    (
        "--stdin is not carried to another host; leave out --on",
        "--stdin wird nicht auf einen anderen Host übertragen; lass --on weg",
    ),
    (
        "cannot open the input of the Job: {error}",
        "die Eingabe des Jobs lässt sich nicht öffnen: {error}",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(english, _)| *english == key)
            .map_or(key, |(_, german)| *german)
            .to_owned()
    } else {
        key.to_owned()
    }
}
