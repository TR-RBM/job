const CATALOGUE: &[(&str, &str)] = &[
    ("user", "Benutzer"),
    ("system", "System"),
    ("JOB_RUNTIME_DIR", "JOB_RUNTIME_DIR"),
    (
        "system runtime directory",
        "Laufzeitverzeichnis des Systems",
    ),
    (
        "JOB_STATE_DIR without JOB_RUNTIME_DIR",
        "JOB_STATE_DIR ohne JOB_RUNTIME_DIR",
    ),
    ("XDG_RUNTIME_DIR", "XDG_RUNTIME_DIR"),
    (
        "state directory, XDG_RUNTIME_DIR is not set",
        "Zustandsverzeichnis, XDG_RUNTIME_DIR ist nicht gesetzt",
    ),
    ("JOB_CGROUP_ROOT", "JOB_CGROUP_ROOT"),
    ("[cgroup] root", "[cgroup] root"),
    ("delegated own cgroup", "delegierte eigene cgroup"),
    (
        "legacy /sys/fs/cgroup/exec",
        "alter Pfad /sys/fs/cgroup/exec",
    ),
    (
        "changing the socket group or the cgroup root requires a daemon restart",
        "starte den Daemon neu, um die Socket-Gruppe oder die cgroup-Wurzel zu ändern",
    ),
    (
        "usage: jobd [--system] [--foreground]",
        "Aufruf: jobd [--system] [--foreground]",
    ),
    (
        "Runs the job service in the foreground and logs to standard error. See jobd(8).",
        "Startet den job-Dienst im Vordergrund und schreibt sein Protokoll auf die Standardfehlerausgabe. Siehe jobd(8).",
    ),
    (
        "jobd: unknown option {option}",
        "jobd: unbekannte Option {option}",
    ),
    (
        "another job service already serves the runtime directory {path}; its state directory is {state}; stop it first, or give this one its own JOB_RUNTIME_DIR",
        "ein anderer job-Dienst bedient das Laufzeitverzeichnis {path} schon; sein Zustandsverzeichnis ist {state}; halte ihn zuerst an oder gib diesem ein eigenes JOB_RUNTIME_DIR",
    ),
    (
        "the runtime directory {path} belongs to user {owner}, not to this service",
        "das Laufzeitverzeichnis {path} gehört Benutzer {owner}, nicht diesem Dienst",
    ),
    (
        "a group-shared socket needs a runtime directory apart from the private state directory; set JOB_RUNTIME_DIR or XDG_RUNTIME_DIR, or run with --system",
        "ein mit einer Gruppe geteilter Socket braucht ein Laufzeitverzeichnis außerhalb des privaten Zustandsverzeichnisses; setze JOB_RUNTIME_DIR oder XDG_RUNTIME_DIR, oder starte mit --system",
    ),
    (
        "{mode} service, state {state}, socket {socket}, chosen by {rule}",
        "{mode}-Dienst, Zustand {state}, Socket {socket}, gewählt durch {rule}",
    ),
    ("shared with group {group}", "geteilt mit Gruppe {group}"),
    (
        "the socket group {group} does not exist; create it or correct [socket] group in the configuration",
        "die Socket-Gruppe {group} gibt es nicht; lege sie an oder berichtige [socket] group in der Konfiguration",
    ),
    (
        "refused: the caller runs as a different user",
        "abgelehnt: der Aufrufer läuft als anderer Benutzer",
    ),
    (
        "refused: the caller is neither the service user nor a member of group {group}",
        "abgelehnt: der Aufrufer ist weder der Dienstbenutzer noch Mitglied der Gruppe {group}",
    ),
    (
        "cannot give {path} to the socket group {group}: {error}; the service user must be a member of that group",
        "{path} lässt sich der Socket-Gruppe {group} nicht übergeben: {error}; der Dienstbenutzer muss Mitglied dieser Gruppe sein",
    ),
    (
        "cgroup root {root}, chosen by {rule}",
        "cgroup-Wurzel {root}, gewählt durch {rule}",
    ),
    (
        "no cgroup root, processes are watched instead",
        "keine cgroup-Wurzel, Prozesse werden stattdessen beobachtet",
    ),
    (
        "{root} holds {count} other processes (first: {pid}); a cgroup root must hold only the service",
        "{root} enthält {count} andere Prozesse (der erste: {pid}); eine cgroup-Wurzel darf nur den Dienst enthalten",
    ),
    (
        "{rule} names {root}, which is not a writable cgroup holding this service",
        "{rule} nennt {root}; das ist keine beschreibbare cgroup, in der dieser Dienst läuft",
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
