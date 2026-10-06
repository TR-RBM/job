const CATALOGUE: &[(&str, &str)] = &[
    (
        "not located, and not used yet",
        "nicht auffindbar und noch nicht benutzt",
    ),
    (
        "{path} is longer than a socket address allows ({limit} bytes)",
        "{path} ist länger, als eine Socket-Adresse erlaubt ({limit} Bytes)",
    ),
    (
        "choose a shorter JOB_RUNTIME_DIR or JOB_STATE_DIR",
        "wähle ein kürzeres JOB_RUNTIME_DIR oder JOB_STATE_DIR",
    ),
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
    ("unexpected answer", "unerwartete Antwort"),
    ("set", "gesetzt"),
    ("unset", "nicht gesetzt"),
    (
        "{mode} service: configuration {config}, state {state}, runtime {runtime}, cache {cache}",
        "{mode}-Dienst: Konfiguration {config}, Zustand {state}, Laufzeit {runtime}, Cache {cache}",
    ),
    (
        "set the named variable in the environment of the service and of its clients",
        "setze die genannte Variable in der Umgebung des Dienstes und seiner Clients",
    ),
    (
        "the unified hierarchy is mounted at /sys/fs/cgroup",
        "die einheitliche Hierarchie ist unter /sys/fs/cgroup eingehängt",
    ),
    (
        "no unified cgroup hierarchy at /sys/fs/cgroup",
        "unter /sys/fs/cgroup ist keine einheitliche cgroup-Hierarchie",
    ),
    (
        "mount cgroup2 at /sys/fs/cgroup; without it limits are watched and not enforced",
        "hänge cgroup2 unter /sys/fs/cgroup ein; ohne sie werden Grenzen beobachtet und nicht durchgesetzt",
    ),
    (
        "this command runs in {path}; the service may run in another cgroup",
        "dieser Befehl läuft in {path}; der Dienst kann in einer anderen cgroup laufen",
    ),
    (
        "/proc/self/cgroup names no unified cgroup",
        "/proc/self/cgroup nennt keine einheitliche cgroup",
    ),
    (
        "delegate a cgroup to the service: Delegate=yes in its systemd unit, or a directory owned by the service user that holds only the service; see jobd(8)",
        "delegiere dem Dienst eine cgroup: Delegate=yes in seiner systemd-Unit, oder ein Verzeichnis, das dem Dienstbenutzer gehört und nur den Dienst enthält; siehe jobd(8)",
    ),
    (
        "the service manages {root}, chosen by {rule}",
        "der Dienst verwaltet {root}, gewählt durch {rule}",
    ),
    (
        "the service has no cgroup root and watches processes instead",
        "der Dienst hat keine cgroup-Wurzel und beobachtet stattdessen Prozesse",
    ),
    (
        "a service started from here would manage {root}, chosen by {rule}",
        "ein von hier gestarteter Dienst würde {root} verwalten, gewählt durch {rule}",
    ),
    (
        "a service started from here would find no cgroup root and watch processes instead",
        "ein von hier gestarteter Dienst fände keine cgroup-Wurzel und würde stattdessen Prozesse beobachten",
    ),
    (
        "the cgroup root cannot be determined before the configuration loads",
        "die cgroup-Wurzel lässt sich erst bestimmen, wenn die Konfiguration lädt",
    ),
    (
        "correct the configuration first",
        "berichtige zuerst die Konfiguration",
    ),
    (
        "no cgroup root, so no controller is available to Jobs",
        "keine cgroup-Wurzel, also steht Jobs kein Controller zur Verfügung",
    ),
    ("{root} offers {controllers}", "{root} bietet {controllers}"),
    (
        "{root} lacks {controllers}",
        "in {root} fehlen {controllers}",
    ),
    (
        "enable the missing controllers in cgroup.subtree_control of the parent cgroup; limits that need them are refused until then",
        "schalte die fehlenden Controller in cgroup.subtree_control der übergeordneten cgroup ein; bis dahin werden Grenzen abgelehnt, die sie brauchen",
    ),
    (
        "{count} block devices; {limited} accept io.max, {weighted} accept an I/O weight",
        "{count} Blockgeräte; {limited} nehmen io.max an, {weighted} nehmen ein I/O-Gewicht an",
    ),
    ("cgroup.freeze is available", "cgroup.freeze ist verfügbar"),
    (
        "cgroup.freeze is not available to the service",
        "cgroup.freeze steht dem Dienst nicht zur Verfügung",
    ),
    (
        "job suspend and job continue need a delegated cgroup root",
        "job suspend und job continue brauchen eine delegierte cgroup-Wurzel",
    ),
    (
        "/proc/pressure/cpu, memory and io are readable",
        "/proc/pressure/cpu, memory und io sind lesbar",
    ),
    (
        "boot a kernel with CONFIG_PSI and without psi=0; pressure rules and job pressure stay unavailable until then",
        "starte einen Kernel mit CONFIG_PSI und ohne psi=0; bis dahin gibt es keine Druckregeln und kein job pressure",
    ),
    (
        "unprivileged user namespaces are allowed",
        "unprivilegierte User-Namespaces sind erlaubt",
    ),
    (
        "unprivileged user namespaces are not allowed",
        "unprivilegierte User-Namespaces sind nicht erlaubt",
    ),
    (
        "--net none, proxy and tunnel networks need them; an administrator may allow them through the sysctl settings named in jobd(8)",
        "--net none, Proxy- und Tunnelnetze brauchen sie; ein Administrator kann sie über die in jobd(8) genannten sysctl-Einstellungen erlauben",
    ),
    ("Landlock ABI {abi}", "Landlock-ABI {abi}"),
    ("Landlock is not available", "Landlock ist nicht verfügbar"),
    (
        "confined Jobs are refused; enable the Landlock security module in the kernel",
        "eingeschränkte Jobs werden abgelehnt; schalte das Sicherheitsmodul Landlock im Kernel ein",
    ),
    (
        "Jobs that request this control are refused; other Jobs are not affected",
        "Jobs, die diese Steuerung verlangen, werden abgelehnt; andere Jobs sind nicht betroffen",
    ),
    (
        "seccomp filters can be installed",
        "seccomp-Filter lassen sich einrichten",
    ),
    (
        "this architecture has no seccomp table in job",
        "für diese Architektur hat job keine seccomp-Tabelle",
    ),
    (
        "no_new_privs can be queried; this command has it {state}",
        "no_new_privs lässt sich abfragen; bei diesem Befehl ist es {state}",
    ),
    (
        "the kernel knows capabilities 0 to {last}",
        "der Kernel kennt die Capabilities 0 bis {last}",
    ),
    (
        "process file descriptors work",
        "Prozess-Dateideskriptoren funktionieren",
    ),
    (
        "the service does not start without pidfd_open and pidfd_send_signal; use Linux 5.3 or later",
        "ohne pidfd_open und pidfd_send_signal startet der Dienst nicht; nimm Linux 5.3 oder neuer",
    ),
    (
        "every tool for bandwidth limits and job networks is on PATH",
        "alle Werkzeuge für Bandbreitengrenzen und Job-Netze liegen im PATH",
    ),
    ("not on PATH: {tools}", "nicht im PATH: {tools}"),
    (
        "install them to use --bandwidth, proxy and tunnel networks; other Jobs are not affected",
        "installiere sie, um --bandwidth, Proxy- und Tunnelnetze zu nutzen; andere Jobs sind nicht betroffen",
    ),
    ("{path} is valid", "{path} ist gültig"),
    (
        "no file at {path}; the ordinary defaults apply",
        "keine Datei unter {path}; es gelten die gewöhnlichen Standardwerte",
    ),
    (
        "correct the file; job config check FILE validates it without a service",
        "berichtige die Datei; job config check DATEI prüft sie ohne Dienst",
    ),
    (
        "the state directory cannot be located",
        "das Zustandsverzeichnis ist nicht auffindbar",
    ),
    (
        "{path} does not exist yet; the service creates it with mode 0700",
        "{path} gibt es noch nicht; der Dienst legt es mit Modus 0700 an",
    ),
    (
        "{path} is private to the service user; output reaches this user through the service and terminals through the runtime directory",
        "{path} ist dem Dienstbenutzer vorbehalten; die Ausgabe erreicht diesen Benutzer über den Dienst und Terminals über das Laufzeitverzeichnis",
    ),
    (
        "make the state directory a directory the service user owns",
        "mach das Zustandsverzeichnis zu einem Verzeichnis, das dem Dienstbenutzer gehört",
    ),
    ("{path} is not a directory", "{path} ist kein Verzeichnis"),
    (
        "{path} belongs to user {owner}; output reaches this user through the service and terminals through the runtime directory",
        "{path} gehört Benutzer {owner}; die Ausgabe erreicht diesen Benutzer über den Dienst und Terminals über das Laufzeitverzeichnis",
    ),
    ("{path} is not writable", "{path} ist nicht beschreibbar"),
    (
        "{path} has mode {mode}; records and logs are readable by others",
        "{path} hat Modus {mode}; Aufzeichnungen und Protokolle sind für andere lesbar",
    ),
    ("chmod 700 {path}", "chmod 700 {path}"),
    (
        "{path} is present and writable",
        "{path} ist vorhanden und beschreibbar",
    ),
    (
        "stop the service and migrate offline with job state migrate; see the migration guide",
        "halte den Dienst an und migriere offline mit job state migrate; siehe die Migrationsanleitung",
    ),
    (
        "state schema {version} matches this program",
        "Zustandsschema {version} passt zu diesem Programm",
    ),
    (
        "state schema {version}, this program needs {current}",
        "Zustandsschema {version}, dieses Programm braucht {current}",
    ),
    (
        "no schema recorded yet; a new state directory starts at {current}",
        "noch kein Schema vermerkt; ein neues Zustandsverzeichnis beginnt bei {current}",
    ),
    (
        "not readable by this user; the service checks it at start",
        "für diesen Benutzer nicht lesbar; der Dienst prüft es beim Start",
    ),
    (
        "an exec store exists at {path}; the service refuses to choose between it and the new one",
        "unter {path} liegt ein exec-Zustand; der Dienst weigert sich, zwischen ihm und dem neuen zu wählen",
    ),
    (
        "an exec store exists at {path}; the state directory is selected explicitly",
        "unter {path} liegt ein exec-Zustand; das Zustandsverzeichnis ist ausdrücklich gewählt",
    ),
    (
        "no exec store beside the state directory",
        "kein exec-Zustand neben dem Zustandsverzeichnis",
    ),
    (
        "start the service: systemctl start jobd, systemctl --user start jobd, sv up job, or jobd in a terminal",
        "starte den Dienst: systemctl start jobd, systemctl --user start jobd, sv up job, oder jobd in einem Terminal",
    ),
    ("a service holds {path}", "ein Dienst hält {path}"),
    (
        "a process holds {path} but no service answers",
        "ein Prozess hält {path}, aber kein Dienst antwortet",
    ),
    (
        "a service is starting, migrating or hung; look at its log, then restart it through its service manager",
        "ein Dienst startet gerade, migriert oder hängt; sieh in sein Protokoll und starte ihn dann über seinen Dienstverwalter neu",
    ),
    ("nothing holds {path}", "nichts hält {path}"),
    (
        "{path} does not exist; no service has run on this state",
        "{path} gibt es nicht; auf diesem Zustand lief noch kein Dienst",
    ),
    (
        "not readable by this user; the lock lives in the private state directory",
        "für diesen Benutzer nicht lesbar; die Sperre liegt im privaten Zustandsverzeichnis",
    ),
    (
        "the socket path cannot be located",
        "der Pfad des Sockets ist nicht auffindbar",
    ),
    (
        "no socket at {path} ({error}); path chosen by {rule}",
        "kein Socket unter {path} ({error}); Pfad gewählt durch {rule}",
    ),
    ("{path} is not a socket", "{path} ist kein Socket"),
    (
        "remove that file and restart the service",
        "entferne diese Datei und starte den Dienst neu",
    ),
    (
        "{path}, owner {owner}, group {group}, mode {mode}; path chosen by {rule}",
        "{path}, Besitzer {owner}, Gruppe {group}, Modus {mode}; Pfad gewählt durch {rule}",
    ),
    (
        "the configuration asks for mode {mode}; restart the service so that it sets the socket up again",
        "die Konfiguration verlangt Modus {mode}; starte den Dienst neu, damit er den Socket neu einrichtet",
    ),
    (
        "no answer at {path}: {error}",
        "keine Antwort unter {path}: {error}",
    ),
    (
        "service {version}, protocol {protocol}; this command {own_version}, protocol {own_protocol}",
        "Dienst {version}, Protokoll {protocol}; dieser Befehl {own_version}, Protokoll {own_protocol}",
    ),
    (
        "restart the service after installing, so that the service and the command are the same build",
        "starte den Dienst nach dem Installieren neu, damit Dienst und Befehl derselbe Build sind",
    ),
    (
        "the socket is private: only the service user connects",
        "der Socket ist privat: nur der Dienstbenutzer verbindet sich",
    ),
    (
        "members of group {group} control every Job, Queue, Group and terminal; you are not the service user, so run, logs and log get output from the service and attach uses the terminal socket in the runtime directory; job audit and job events read the private state directory and stay with the service user",
        "Mitglieder der Gruppe {group} steuern jeden Job, jede Queue, jede Gruppe und jedes Terminal; du bist nicht der Dienstbenutzer, deshalb bekommen run, logs und log die Ausgabe vom Dienst und attach nutzt den Terminal-Socket im Laufzeitverzeichnis; job audit und job events lesen das private Zustandsverzeichnis und bleiben beim Dienstbenutzer",
    ),
    (
        "members of group {group} control every Job, Queue, Group and terminal; they get output from the service and terminals through the runtime directory; job audit and job events read the private state directory and stay with the service user",
        "Mitglieder der Gruppe {group} steuern jeden Job, jede Queue, jede Gruppe und jedes Terminal; sie bekommen die Ausgabe vom Dienst und Terminals über das Laufzeitverzeichnis; job audit und job events lesen das private Zustandsverzeichnis und bleiben beim Dienstbenutzer",
    ),
    ("systemd is the init system", "systemd ist das Init-System"),
    (
        "this release was never run under systemd; its unit files are a starting point",
        "diese Version lief nie unter systemd; ihre Unit-Dateien sind ein Ausgangspunkt",
    ),
    (
        "verify once: start the unit, run job doctor, run a Job, systemctl restart jobd, and confirm that the Job kept running; see the systemd part of the administration guide",
        "prüfe einmal: starte die Unit, führe job doctor aus, starte einen Job, führe systemctl restart jobd aus und stelle fest, dass der Job weiterlief; siehe den systemd-Teil des Administrationshandbuchs",
    ),
    (
        "this command runs inside a systemd unit",
        "dieser Befehl läuft in einer systemd-Unit",
    ),
    (
        "runit service directory {path}",
        "runit-Dienstverzeichnis {path}",
    ),
    (
        "neither systemd nor runit found; run jobd in the foreground under your supervisor",
        "weder systemd noch runit gefunden; starte jobd im Vordergrund unter deinem Dienstverwalter",
    ),
    ("runit runs on this host", "auf diesem Rechner läuft runit"),
    (
        "lingering is enabled for {user}",
        "Lingering ist für {user} eingeschaltet",
    ),
    (
        "lingering is not enabled for {user}; a user service and its Jobs stop at the last logout",
        "Lingering ist für {user} nicht eingeschaltet; ein Benutzerdienst und seine Jobs enden mit der letzten Abmeldung",
    ),
    (
        "loginctl enable-linger {user}",
        "loginctl enable-linger {user}",
    ),
    (
        "usage: job doctor [--json] [--system]",
        "Aufruf: job doctor [--json] [--system]",
    ),
    ("Do:", "Zu tun:"),
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
