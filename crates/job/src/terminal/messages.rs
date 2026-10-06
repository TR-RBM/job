const CATALOGUE: &[(&str, &str)] = &[
    ("invalid terminal size", "ungültige Terminalgröße"),
    ("terminal slave is closed", "das Terminal ist geschlossen"),
    (
        "terminal input buffer exceeded",
        "der Terminal-Eingabepuffer ist voll",
    ),
    (
        "terminal frame too large",
        "die Terminalnachricht ist zu groß",
    ),
    ("invalid terminal response", "ungültige Terminalantwort"),
    (
        "attach needs a terminal on stdin and stdout; use job submit --pty for a detached terminal",
        "attach braucht ein Terminal für stdin und stdout; mit job submit --pty startest du ein Terminal im Hintergrund",
    ),
    (
        "job {id} has finished; use job log {id} full",
        "Job {id} ist beendet; mit job log {id} full liest du seine Ausgabe",
    ),
    (
        "unexpected attach response: {error}",
        "unerwartete Attach-Antwort: {error}",
    ),
    (
        "[job] terminal {id}: shared input; detach with {key} then d",
        "[job] Terminal {id}: gemeinsame Eingabe; trenne dich mit {key} und danach d",
    ),
    (
        "[job] terminal {id}: shared input; no detach key is set, close this terminal to detach",
        "[job] Terminal {id}: gemeinsame Eingabe; es ist keine Trenntaste gesetzt, schließe dieses Terminal, um dich zu trennen",
    ),
    ("Ctrl-{key}", "Strg-{key}"),
    (
        "detach key `{key}` is not ctrl-LETTER, ctrl-], ctrl-\\, ctrl-^, ctrl-_ or none",
        "die Trenntaste `{key}` ist weder ctrl-BUCHSTABE, ctrl-], ctrl-\\, ctrl-^, ctrl-_ noch none",
    ),
    (
        "usage: job attach ID [--detach-key KEY|none]",
        "Aufruf: job attach ID [--detach-key KEY|none]",
    ),
    (
        "Terminal detach key: job attach ID --detach-key KEY|none; environment JOB_DETACH_KEY; service configuration [terminal] detach_key",
        "Trenntaste im Terminal: job attach ID --detach-key KEY|none; Umgebungsvariable JOB_DETACH_KEY; Dienstkonfiguration [terminal] detach_key",
    ),
    (
        "KEY is ctrl-LETTER, ctrl-], ctrl-\\, ctrl-^ or ctrl-_; the default is ctrl-]. Press KEY then d to detach; press KEY twice to send it once to the program. The option wins over the environment, the environment over the configuration. With none there is no detach key: you detach only by closing your terminal.",
        "KEY ist ctrl-BUCHSTABE, ctrl-], ctrl-\\, ctrl-^ oder ctrl-_; der Standard ist ctrl-]. Drücke KEY und danach d, um dich zu trennen; drücke KEY zweimal, um die Taste einmal an das Programm zu senden. Die Option gilt vor der Umgebungsvariable, die Umgebungsvariable vor der Konfiguration. Mit none gibt es keine Trenntaste: du trennst dich nur, indem du dein Terminal schließt.",
    ),
    (
        "[job] detached from {id}; attach again with: job attach {id}",
        "[job] von {id} getrennt; mit job attach {id} verbindest du dich erneut",
    ),
    (
        "terminal connection ended: {error}; reconnect with job attach {id}",
        "Terminalverbindung beendet: {error}; mit job attach {id} verbindest du dich erneut",
    ),
    (
        "job {id} has no terminal; start it with --pty",
        "Job {id} hat kein Terminal; starte ihn mit --pty",
    ),
    (
        "--pty is local to a job service; connect with SSH and run job there",
        "--pty gilt lokal für einen Job-Dienst; verbinde dich über SSH und führe job dort aus",
    ),
    (
        "--pty cannot be combined with --json",
        "--pty lässt sich nicht mit --json kombinieren",
    ),
    (
        "the running daemon does not support this terminal protocol; update it before submitting a PTY job",
        "der laufende Daemon unterstützt dieses Terminalprotokoll nicht; aktualisiere ihn, bevor du einen PTY-Job startest",
    ),
    (
        "cannot create terminal: {error}",
        "Terminal kann nicht erstellt werden: {error}",
    ),
    (
        "cannot attach terminal: {error}",
        "Terminal kann nicht verbunden werden: {error}",
    ),
    (
        "\n[job: terminal log omitted {lost} bytes because storage could not keep up]\n",
        "\n[job: im Terminalprotokoll fehlen {lost} Bytes, weil der Datenträger nicht mithalten konnte]\n",
    ),
];

pub fn help() -> String {
    [
        "Terminal detach key: job attach ID --detach-key KEY|none; environment JOB_DETACH_KEY; service configuration [terminal] detach_key",
        "KEY is ctrl-LETTER, ctrl-], ctrl-\\, ctrl-^ or ctrl-_; the default is ctrl-]. Press KEY then d to detach; press KEY twice to send it once to the program. The option wins over the environment, the environment over the configuration. With none there is no detach key: you detach only by closing your terminal.",
    ]
    .map(|line| message(line, &[]))
    .join("\n")
}

pub fn message(english: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    translate(english, values, locale.starts_with("de"))
}

fn translate(english: &str, values: &[(&str, String)], german: bool) -> String {
    let template = if german {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn german_detach_message_preserves_the_command() {
        let message = translate(
            "[job] detached from {id}; attach again with: job attach {id}",
            &[("id", "42".to_owned())],
            true,
        );
        assert!(message.contains("getrennt"));
        assert!(message.contains("job attach 42"));
    }
}
