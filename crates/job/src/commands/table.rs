use super::{Arity, Check, Command, Group, Operand, Opt, Section, Value};

pub const HELP_OPTION: &str = "--help";
pub const FORMAT_OPTION: &str = "--format";

const fn flag(long: &'static str, help: &'static str) -> Opt {
    Opt {
        long,
        short: None,
        arity: Arity::Flag,
        value: Value::Free,
        meta: "",
        help,
    }
}

const fn one(long: &'static str, meta: &'static str, value: Value, help: &'static str) -> Opt {
    Opt {
        long,
        short: None,
        arity: Arity::One,
        value,
        meta,
        help,
    }
}

const fn many(long: &'static str, meta: &'static str, value: Value, help: &'static str) -> Opt {
    Opt {
        long,
        short: None,
        arity: Arity::Many,
        value,
        meta,
        help,
    }
}

const fn short(option: Opt, short: &'static str) -> Opt {
    Opt {
        long: option.long,
        short: Some(short),
        arity: option.arity,
        value: option.value,
        meta: option.meta,
        help: option.help,
    }
}

const fn group(heading: &'static str, options: &'static [Opt]) -> Group {
    Group {
        heading,
        legacy: false,
        options,
    }
}

const fn required(name: &'static str, value: Value, help: &'static str) -> Operand {
    Operand {
        name,
        value,
        required: true,
        repeat: false,
        help,
    }
}

const fn optional(name: &'static str, value: Value, help: &'static str) -> Operand {
    Operand {
        name,
        value,
        required: false,
        repeat: false,
        help,
    }
}

const fn repeated(name: &'static str, value: Value, help: &'static str) -> Operand {
    Operand {
        name,
        value,
        required: false,
        repeat: true,
        help,
    }
}

const NET: Value = Value::Network;
const STREAMS: Value = Value::Words(&[&["all", "stdout", "stderr", "combined", "diagnostic"]]);
const FORMATS: Value = Value::Words(&[super::output::FORMATS]);
const TABLES: Value = Value::Words(&[super::output::TABULAR]);
const STATES: Value = Value::Words(&[crate::cli2::listing::STATES]);
const PROFILES: Value = Value::Words(&[&["ordinary", "legacy"]]);
const PRESSURES: Value = Value::Words(&[&["cpu", "memory", "io"]]);
const BOOLEAN: Value = Value::Words(&[&["true", "false"]]);
const BACKFILL: Value = Value::Words(&[&["off", "conservative"]]);
const FAIR: Value = Value::Words(&[&["off", "cpu-request-time", "memory-request-time"]]);
const CONFIRM: Value = Value::Words(&[&["yes", "inherit"]]);
const INHERIT: Value = Value::Words(&[&["inherit"]]);
const UNLIMITED: Value = Value::Words(&[&["unlimited"]]);
const NUMA: Value = Value::Words(&[&[
    "inherit",
    "default",
    "local",
    "bind:",
    "interleave:",
    "preferred:",
]]);
const RLIMIT: Value = Value::Words(&[&[
    "as=",
    "core=",
    "cpu=",
    "data=",
    "fsize=",
    "memlock=",
    "msgqueue=",
    "nice=",
    "nofile=",
    "nproc=",
    "rtprio=",
    "rttime=",
    "sigpending=",
    "stack=",
]]);
const CAPABILITIES: Value = Value::Words(&[&["all", "inherit"], crate::security::CAPABILITIES]);
const SYSCALLS: Value = Value::Words(&[&["inherit"], crate::security::SYSCALLS]);
const NAMESPACES: Value = Value::Words(&[&["inherit"], crate::isolation::KINDS]);
const ROOT: Value = Value::Words(&[&["read-only", "inherit"]]);
const DETACH: Value = Value::Words(&[&["none", "ctrl-]", "ctrl-a", "ctrl-b", "ctrl-g", "ctrl-t"]]);
const SHELLS: Value = Value::Words(&[&["bash", "fish", "zsh"]]);
const QUERIES: Value = Value::Words(&[&["errors", "grep", "lines", "tail", "full"]]);
const KINDS: Value = Value::Words(&[super::completion::KINDS]);

macro_rules! controls {
    ($direct:ident, $default:ident; $(($name:literal, $meta:literal, $value:expr, $help:literal)),* $(,)?) => {
        const $direct: &[Opt] = &[$(one(concat!("--", $name), $meta, $value, $help)),*];
        const $default: &[Opt] = &[$(one(concat!("--job-", $name), $meta, $value, $help)),*];
    };
}

controls!(REQUESTS, JOB_REQUESTS;
    ("cpu-request", "N", Value::Free, "CPUs counted at admission"),
    ("memory-request", "SIZE", Value::Free, "memory counted at admission"),
);

controls!(LIMITS, JOB_LIMITS;
    ("cpu-limit", "N|unlimited", UNLIMITED, "CPU time limit in CPUs"),
    ("cpu-weight", "N", Value::Free, "CPU weight from 1 through 10000"),
    ("memory-high", "SIZE|unlimited", UNLIMITED, "memory throttling threshold"),
    ("memory-max", "SIZE|unlimited", UNLIMITED, "hard memory limit"),
    ("memory-swap-max", "SIZE|unlimited", UNLIMITED, "swap limit"),
    ("pids-max", "N|unlimited", UNLIMITED, "limit on the number of processes and threads"),
    ("io-max", "MAJOR:MINOR,KEY=VALUE...", Value::Free, "I/O rate limits of one block device"),
    ("io-weight", "MAJOR:MINOR=WEIGHT", Value::Free, "I/O weight on one block device"),
    ("io-bfq-weight", "MAJOR:MINOR=WEIGHT", Value::Free, "BFQ weight on one block device"),
);

controls!(LAUNCH, JOB_LAUNCH;
    ("cpu-affinity", "LIST|inherit", INHERIT, "CPUs the processes may run on"),
    ("numa-policy", "POLICY", NUMA, "NUMA memory policy"),
    ("rlimit", "NAME=SOFT[:HARD]", RLIMIT, "one resource limit of the processes"),
    ("no-new-privs", "yes|inherit", CONFIRM, "forbid gaining privileges through exec"),
    ("cap-drop", "NAME,...|all|inherit", CAPABILITIES, "capabilities removed before exec"),
    ("seccomp-deny", "NAME,...|inherit", SYSCALLS, "system calls refused with an error"),
    ("namespaces", "KIND,...|inherit", NAMESPACES, "namespaces created for the processes"),
    ("root", "read-only|inherit", ROOT, "make the file system tree read-only"),
    ("private-tmp", "yes|inherit", CONFIRM, "give the processes their own empty temporary directories"),
    ("writable", "PATH,...|inherit", INHERIT, "paths that stay writable under a read-only root"),
    ("output-head", "SIZE", Value::Free, "bytes kept from the start of each output stream"),
    ("output-tail", "SIZE", Value::Free, "bytes kept from the end of each output stream"),
);

const JSON: Opt = flag("--json", "print the result as JSON");
const DRY_RUN: Opt = flag("--dry-run", "show what would happen and change nothing");
const RECURSIVE: Opt = flag("--recursive", "include everything below the path");
const ALLOW_LOST: Opt = flag(
    "--allow-lost",
    "accept Jobs whose execution state is unknown",
);
const TIMEOUT: Opt = one(
    "--timeout",
    "DURATION",
    Value::Free,
    "how long to wait, for example 30s",
);
const QUEUE_OPTION: Opt = short(
    one("--queue", "PATH", Value::QueuePath, "the Queue to use"),
    "-q",
);

const SYSTEM: Opt = flag("--system", "use the system service, as JOB_SYSTEM=1 does");

pub const GLOBAL: Group = group("Global options", &[SYSTEM]);

pub const HELP: Group = group(
    "Options",
    &[short(flag(HELP_OPTION, "show this help"), "-h")],
);

pub const FORMAT: Group = group(
    "Options",
    &[one(
        FORMAT_OPTION,
        "text|json",
        FORMATS,
        "json prints one versioned envelope with schema_version, kind and data",
    )],
);

pub const FORMAT_TSV: Group = group(
    "Options",
    &[one(
        FORMAT_OPTION,
        "text|json|tsv",
        TABLES,
        "json prints one versioned envelope; tsv prints tab-separated columns under a header line",
    )],
);

const LABEL_FILTER: Opt = many(
    "--label",
    "KEY=VALUE",
    Value::Free,
    "only entries that carry this label; repeat to require several",
);

const LABEL: Opt = many(
    "--label",
    "KEY=VALUE",
    Value::Free,
    "a label for classification; it never affects scheduling or policy",
);

const SUBMIT: Group = group(
    "Options",
    &[
        QUEUE_OPTION,
        LABEL,
        one(
            "--idempotency-key",
            "KEY",
            Value::Free,
            "a repeated submission with this key returns the first Job",
        ),
        one("--dir", "DIR", Value::Dir, "the working directory"),
        one(
            "--session",
            "NAME",
            Value::Free,
            "the label of the calling session",
        ),
        one(
            "--shell",
            "SHELL",
            Value::Program,
            "run SHELL -c with the first argument as the script",
        ),
        flag(
            "--legacy-shell",
            "let Bash interpret a single argument, as before",
        ),
        flag("--pty", "give the Job a persistent shared terminal"),
        flag(
            "--summary",
            "print the final diagnostic summary instead of live output",
        ),
        JSON,
        one(
            "--budget",
            "DURATION|none",
            Value::Words(&[&["none"]]),
            "how long run waits before it returns 75",
        ),
        one(
            "--time",
            "DURATION",
            Value::Free,
            "stop the Job after this long",
        ),
        one(
            "--priority",
            "N",
            Value::Free,
            "admission priority from -1000 through 1000",
        ),
        one(
            "--execution-profile",
            "NAME@REV|none",
            Value::Preset,
            "a versioned execution profile",
        ),
        one(
            "--class",
            "NAME@REV|none",
            Value::Preset,
            "a versioned scheduling class",
        ),
    ],
);

const RESERVE: Group = group(
    "Reservation",
    &[
        one("--cores", "N", Value::Free, "cores to reserve"),
        one(
            "--mem",
            "SIZE",
            Value::Free,
            "memory to reserve, for example 4G",
        ),
        one(
            "--pids",
            "N",
            Value::Free,
            "earlier spelling: reserve N processes and threads and limit the Job to them",
        ),
        one(
            "--write-budget",
            "SIZE",
            Value::Free,
            "bytes it may write before the service stops it; not a file system quota",
        ),
        one(
            "--disk",
            "SIZE",
            Value::Free,
            "earlier spelling of --write-budget",
        ),
        many(
            "--device",
            "NAME",
            Value::Free,
            "an exclusive device to hold",
        ),
    ],
);

const PLACE: Group = group(
    "Placement and network",
    &[
        one(
            "--net",
            "default|none|URL|ns:NAME|profile:NAME",
            NET,
            "the host network, none, a proxy or tunnel, a configured namespace or a named profile",
        ),
        one(
            "--net-secret-file",
            "FILE",
            Value::File,
            "a private file with user:password for the proxy",
        ),
        one(
            "--bandwidth",
            "RATE",
            Value::Free,
            "network rate each way, for example 5Mbit",
        ),
        one(
            "--monitor",
            "M",
            Value::Free,
            "the monitor its windows belong on",
        ),
        one(
            "--on",
            "USER@HOST",
            Value::Free,
            "run on the job service of another host over ssh",
        ),
        one(
            "--ssh-key",
            "FILE",
            Value::File,
            "the ssh key used with --on",
        ),
        many(
            "--ssh-option",
            "KEY=VALUE",
            Value::Free,
            "an ssh option used with --on",
        ),
        many(
            "--send",
            "PATH",
            Value::File,
            "copy PATH to the other host before the start",
        ),
        many(
            "--fetch",
            "PATH",
            Value::File,
            "copy PATH back from the other host after the end",
        ),
        flag("--confine", "let the Job write only in its own tree"),
        many(
            "--allow-write",
            "PATH",
            Value::File,
            "also allow writing under PATH; implies --confine",
        ),
    ],
);

const INTERRUPT: Value = Value::Words(&[&["forward", "detach", "cancel"]]);

const ATTACHED: Group = group(
    "Options",
    &[
        flag(
            "--stdin",
            "pass the standard input of this command to the Job and close it at its end",
        ),
        one(
            "--on-interrupt",
            "forward|detach|cancel",
            INTERRUPT,
            "what the first SIGINT or SIGTERM does: pass it to the Job, leave the Job running, or cancel it",
        ),
    ],
);

const RUN: &[Group] = &[
    SUBMIT,
    ATTACHED,
    RESERVE,
    PLACE,
    group("Resource controls", REQUESTS),
    group("Resource controls", LIMITS),
    group("Process and security controls", LAUNCH),
];

const RESOURCES: &[Group] = &[
    SUBMIT,
    RESERVE,
    PLACE,
    group("Resource controls", REQUESTS),
    group("Resource controls", LIMITS),
    group("Process and security controls", LAUNCH),
];

const UPDATE: &[Group] = &[
    group(
        "Options",
        &[
            flag(
                "--allow-oom",
                "allow lowering memory.max; processes may be killed",
            ),
            DRY_RUN,
            JSON,
        ],
    ),
    group("Resource controls", LIMITS),
];

const SETTINGS: &[Group] = &[
    group(
        "Options",
        &[
            JSON,
            LABEL,
            one(
                "--max-running",
                "N|unlimited",
                UNLIMITED,
                "how many of its Jobs run at once",
            ),
            one(
                "--parallel",
                "N|all",
                Value::Words(&[&["all"]]),
                "earlier spelling of --max-running",
            ),
            one(
                "--cores",
                "N",
                Value::Free,
                "cores its Jobs hold together at most",
            ),
            one(
                "--mem",
                "SIZE",
                Value::Free,
                "memory its Jobs hold together at most",
            ),
            one(
                "--dir",
                "DIR",
                Value::Dir,
                "where its Jobs run unless a Job says --dir",
            ),
            one(
                "--net",
                "default|none|URL|ns:NAME|profile:NAME",
                NET,
                "the host network, none, a proxy or tunnel, a configured namespace or a named profile",
            ),
            one(
                "--net-secret-file",
                "FILE",
                Value::File,
                "a private file with user:password for the proxy",
            ),
            one(
                "--pressure",
                "FILE",
                Value::File,
                "pressure rules as a JSON file",
            ),
        ],
    ),
    group(
        "Admission",
        &[
            one(
                "--priority",
                "N",
                Value::Free,
                "admission priority from -1000 through 1000",
            ),
            one(
                "--priority-min",
                "N",
                Value::Free,
                "lowest priority a Job may ask for",
            ),
            one(
                "--priority-max",
                "N",
                Value::Free,
                "highest priority a Job may ask for",
            ),
            one(
                "--aging",
                "DURATION",
                Value::Free,
                "aging interval for waiting Jobs",
            ),
            one(
                "--strict-fifo",
                "true|false",
                BOOLEAN,
                "start Jobs strictly in submission order",
            ),
            one(
                "--backfill",
                "off|conservative",
                BACKFILL,
                "let small Jobs start ahead without delaying others",
            ),
            one(
                "--fair-share",
                "BASIS|off",
                FAIR,
                "fair share between the children of a Group",
            ),
            one(
                "--share-weight",
                "N",
                Value::Free,
                "weight of this child from 1 through 10000",
            ),
        ],
    ),
    group("Controls for the whole subtree", LIMITS),
    group(
        "Defaults for each Job",
        &[
            one(
                "--job-execution-profile",
                "NAME@REV|none",
                Value::Preset,
                "a versioned execution profile",
            ),
            one(
                "--job-class",
                "NAME@REV|none",
                Value::Preset,
                "a versioned scheduling class",
            ),
        ],
    ),
    group("Defaults for each Job", JOB_REQUESTS),
    group("Defaults for each Job", JOB_LIMITS),
    group("Defaults for each Job", JOB_LAUNCH),
];

const EARLIER: Group = Group {
    heading: "Earlier Queue settings",
    legacy: true,
    options: &[
        one(
            "--parallel",
            "N|all",
            Value::Words(&[&["all"]]),
            "earlier spelling of --max-running",
        ),
        one(
            "--cores",
            "N",
            Value::Free,
            "cores its Jobs hold together at most",
        ),
        one(
            "--mem",
            "SIZE",
            Value::Free,
            "memory its Jobs hold together at most",
        ),
        one(
            "--dir",
            "DIR",
            Value::Dir,
            "where its Jobs run unless a Job says --dir",
        ),
        one(
            "--net",
            "default|none|URL|ns:NAME|profile:NAME",
            NET,
            "the host network, none, a proxy or tunnel, a configured namespace or a named profile",
        ),
        one(
            "--net-secret-file",
            "FILE",
            Value::File,
            "a private file with user:password for the proxy",
        ),
        one(
            "--bandwidth",
            "RATE",
            Value::Free,
            "all its Jobs together, each way",
        ),
        one(
            "--job-bandwidth",
            "RATE",
            Value::Free,
            "each of its Jobs unless a Job says --bandwidth",
        ),
        one(
            "--on",
            "USER@HOST",
            Value::Free,
            "run on the job service of another host over ssh",
        ),
        one(
            "--ssh-key",
            "FILE",
            Value::File,
            "the ssh key used with --on",
        ),
        many(
            "--ssh-option",
            "KEY=VALUE",
            Value::Free,
            "an ssh option used with --on",
        ),
        one(
            "--monitor",
            "M",
            Value::Free,
            "the monitor its windows belong on",
        ),
        one(
            "--host-cores",
            "N",
            Value::Free,
            "cores left to this host while one of its Jobs runs",
        ),
    ],
};

const PLAIN: &[(&str, &str)] = &[("0", "success"), ("125", "usage error or service failure")];

const OUTCOME: &[(&str, &str)] = &[
    ("0", "the Job succeeded"),
    (
        "N",
        "the exit status of the Job itself; 128+S when signal S ended it",
    ),
    ("1", "the Job was cancelled or stopped by the service"),
    (
        "75",
        "the deadline passed while the Job was pending or running, or the service stayed unavailable",
    ),
    (
        "125",
        "service failure, lost Job, start error or usage error",
    ),
];

const STANDING: &[(&str, &str)] = &[
    ("0", "the Job succeeded"),
    ("N", "the exit status of the completed Job"),
    ("1", "the Job was cancelled or stopped by the service"),
    ("75", "the Job is still pending or running"),
    ("125", "usage error or service failure"),
];

const CONTROL: &[(&str, &str)] = &[
    ("0", "success"),
    ("1", "the request was refused or a member failed"),
    ("75", "the change is still pending"),
    ("125", "usage error or service failure"),
];

const TERMINAL: &[(&str, &str)] = &[
    ("0", "the Job succeeded or the client detached"),
    (
        "N",
        "the exit status of the Job itself; 128+S when signal S ended it",
    ),
    ("125", "usage error or service failure"),
];

const SILENT: &[(&str, &str)] = &[("0", "always, with no output on any error")];

const ID: Operand = required("ID", Value::JobId, "the number of a Job");
const ANY_ID: Operand = required("ID", Value::AnyJobId, "the number of a Job");
const SET_HELP: &str = "one Job, an inclusive range such as 1-10, or a comma list such as 1,4,7-9";
const SET_META: &str = "ID[-ID][,ID...]";
const ID_SET: Operand = Operand {
    name: SET_META,
    value: Value::JobIdSet,
    required: true,
    repeat: true,
    help: SET_HELP,
};
const ENDED_SET: Operand = Operand {
    name: SET_META,
    value: Value::EndedJobIdSet,
    required: true,
    repeat: true,
    help: SET_HELP,
};
const SETS: &str = "With one ID the answer is what it always was. With a range, a list or several operands the service resolves the set in one request and answers one line per Job, or one summary line when more than 20 Jobs behaved the same; --json prints schema_version, kind and one result per ID. IDs inside a range that do not exist are skipped; a missing ID named on its own is reported. A set names at most 10000 IDs.";
const SET_EXITS: &[(&str, &str)] = &[
    ("0", "success"),
    (
        "1",
        "with a set: a Job was refused or failed, an ID named on its own does not exist, or no Job of the set exists",
    ),
    ("75", "with a set: a change is still pending"),
    (
        "125",
        "usage error or service failure; with one ID also a refusal",
    ),
];

const BASE: Command = Command {
    path: &[],
    section: Section::Execution,
    summary: "",
    groups: &[],
    operands: &[],
    trailing_command: false,
    end_of_options: false,
    check: Check::Dispatch,
    exits: PLAIN,
    kind: None,
    available: true,
    hidden: false,
    notes: &[],
};

const LITERAL: &str = "Arguments after -- execute literally; use --shell SHELL or an explicit sh -c command for shell syntax.";

pub static COMMANDS: &[Command] = &[
    Command {
        path: &["run"],
        summary: "execute a command and wait for it",
        groups: RUN,
        trailing_command: true,
        exits: OUTCOME,
        kind: Some("run"),
        notes: &[
            LITERAL,
            "A second SIGINT or SIGTERM, a hangup or a closed output makes run return 75 and leaves the Job running; nothing but --on-interrupt cancel cancels it.",
        ],
        ..BASE
    },
    Command {
        path: &["submit"],
        summary: "submit a command and print the Job ID",
        groups: RESOURCES,
        trailing_command: true,
        kind: Some("submit"),
        notes: &[
            LITERAL,
            "submit --json prints the Job ID as before; --format json wraps the Job record.",
        ],
        ..BASE
    },
    Command {
        path: &["create"],
        summary: "create held work without starting it",
        groups: RESOURCES,
        trailing_command: true,
        kind: Some("create"),
        notes: &[LITERAL],
        ..BASE
    },
    Command {
        path: &["edit"],
        summary: "replace the command and environment of held or queued work",
        groups: RESOURCES,
        operands: &[ID],
        trailing_command: true,
        kind: Some("edit"),
        notes: &[LITERAL],
        ..BASE
    },
    Command {
        path: &["release"],
        summary: "release held work for scheduling",
        groups: &[group("Options", &[DRY_RUN, JSON])],
        operands: &[ID_SET],
        exits: SET_EXITS,
        kind: Some("release"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["retry"],
        summary: "start a new attempt of a completed Job",
        groups: &[group(
            "Options",
            &[
                flag("--hold", "create the attempt held"),
                flag(
                    "--current-env",
                    "use the environment of this shell instead of the saved one",
                ),
                QUEUE_OPTION,
                ALLOW_LOST,
                DRY_RUN,
                JSON,
            ],
        )],
        operands: &[ENDED_SET],
        exits: SET_EXITS,
        kind: Some("retry"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["wait"],
        summary: "wait quietly for a Job and return its outcome",
        groups: &[group(
            "Options",
            &[
                TIMEOUT,
                flag(
                    "--summary",
                    "print the final diagnostic summary instead of live output",
                ),
                JSON,
            ],
        )],
        operands: &[ID_SET],
        end_of_options: true,
        exits: OUTCOME,
        kind: Some("wait"),
        notes: &[
            "With a set, wait waits quietly until every Job of it has ended and prints one line per Job; --timeout bounds the whole wait, and the exit status is the first one in ID order that is not 0, where a Job still running at the deadline counts as 75 and a missing ID named on its own as 1.",
            "A waiting client that ends does not cancel the Job.",
        ],
        ..BASE
    },
    Command {
        path: &["cancel"],
        summary: "cancel a Job",
        groups: &[group(
            "Options",
            &[
                one(
                    "--session",
                    "NAME",
                    Value::Free,
                    "the label of the calling session",
                ),
                DRY_RUN,
                JSON,
            ],
        )],
        operands: &[ID_SET],
        exits: CONTROL,
        kind: Some("cancel"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["suspend"],
        summary: "freeze the processes of a running Job",
        groups: &[group("Options", &[TIMEOUT, DRY_RUN, JSON])],
        operands: &[ID_SET],
        exits: CONTROL,
        kind: Some("suspend"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["continue"],
        summary: "let a suspended Job run again",
        groups: &[group("Options", &[TIMEOUT, DRY_RUN, JSON])],
        operands: &[ID_SET],
        exits: CONTROL,
        kind: Some("continue"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["signal"],
        summary: "send a signal to the processes of a Job",
        groups: &[group(
            "Options",
            &[
                short(
                    one(
                        "--signal",
                        "SIGNAL",
                        Value::Signal,
                        "the signal by name or number; TERM when absent",
                    ),
                    "-s",
                ),
                DRY_RUN,
                JSON,
            ],
        )],
        operands: &[ID_SET],
        notes: &[SETS],
        exits: SET_EXITS,
        kind: Some("signal"),
        ..BASE
    },
    Command {
        path: &["update"],
        summary: "change resource controls of a running Job",
        groups: UPDATE,
        operands: &[ID],
        exits: CONTROL,
        kind: Some("update"),
        notes: &[
            "Preview with --dry-run. Exit 75 means pending: ask with resource-update instead of sending the change again.",
        ],
        ..BASE
    },
    Command {
        path: &["resource-update"],
        summary: "show or abandon a pending resource update",
        groups: &[group(
            "Options",
            &[flag("--abandon", "give up the pending operation"), JSON],
        )],
        operands: &[required(
            "OPERATION",
            Value::Free,
            "the operation ID printed by update",
        )],
        exits: CONTROL,
        kind: Some("resource_update"),
        ..BASE
    },
    Command {
        path: &["reprioritize"],
        summary: "change the admission priority of a waiting Job",
        groups: &[group(
            "Options",
            &[
                one(
                    "--priority",
                    "N",
                    Value::Free,
                    "admission priority from -1000 through 1000",
                ),
                DRY_RUN,
                JSON,
            ],
        )],
        operands: &[ID_SET],
        notes: &[SETS],
        exits: SET_EXITS,
        kind: Some("reprioritize"),
        ..BASE
    },
    Command {
        path: &["move"],
        summary: "move a held or queued Job into another Queue",
        groups: &[group("Options", &[QUEUE_OPTION, DRY_RUN, JSON])],
        operands: &[ID_SET],
        exits: SET_EXITS,
        kind: Some("move"),
        notes: &[
            SETS,
            "The Job keeps its number, its submission time and its waiting credit. The destination must exist and be open, and its constraints and priority bounds are checked again.",
        ],
        ..BASE
    },
    Command {
        path: &["remove"],
        summary: "remove the record of a completed Job",
        groups: &[group("Options", &[DRY_RUN, ALLOW_LOST, JSON])],
        operands: &[ENDED_SET],
        exits: CONTROL,
        kind: Some("remove"),
        notes: &[SETS],
        ..BASE
    },
    Command {
        path: &["attach"],
        section: Section::Interaction,
        summary: "join the shared terminal of a Job started with --pty",
        groups: &[group(
            "Options",
            &[one(
                "--detach-key",
                "KEY|none",
                DETACH,
                "the key that, followed by d, leaves the terminal",
            )],
        )],
        operands: &[ID],
        exits: TERMINAL,
        notes: &[
            "Detach with the detach key, Ctrl-] unless set otherwise, then d; the Job goes on.",
        ],
        ..BASE
    },
    Command {
        path: &["list"],
        section: Section::Inspection,
        summary: "list Jobs; held, queued and running ones unless asked otherwise",
        groups: &[group(
            "Options",
            &[
                flag("--all", "include completed Jobs"),
                one(
                    "--state",
                    "STATE[,STATE...]",
                    STATES,
                    "only Jobs in these states",
                ),
                one(
                    "--limit",
                    "N",
                    Value::Free,
                    "list at most N Jobs, the most recent ones; 200 when absent",
                ),
                LABEL_FILTER,
                QUEUE_OPTION,
                one(
                    "--id",
                    SET_META,
                    Value::JobIdSet,
                    "only these Jobs, in every state unless --state says otherwise",
                ),
                JSON,
            ],
        )],
        kind: Some("list"),
        notes: &[
            "With --all, --state, --label, --limit, --id or --format tsv the listing is a table with one line per Job.",
            "The tsv columns are id, attempt, state, queue, session, priority, submitted_ms, started_ms, finished_ms, exit_status, labels and command; labels and command are JSON.",
        ],
        ..BASE
    },
    Command {
        path: &["show"],
        section: Section::Inspection,
        summary: "show the answer of a Job, or where it stands",
        groups: &[group("Options", &[JSON])],
        operands: &[ANY_ID],
        exits: STANDING,
        kind: Some("show"),
        ..BASE
    },
    Command {
        path: &["status"],
        section: Section::Inspection,
        summary: "earlier spelling of show",
        groups: &[group("Options", &[JSON])],
        operands: &[ANY_ID],
        exits: STANDING,
        kind: Some("status"),
        ..BASE
    },
    Command {
        path: &["explain"],
        section: Section::Inspection,
        summary: "explain why a Job waits, or what a setting of a Queue or Group amounts to",
        groups: &[group("Options", &[JSON])],
        operands: &[
            required(
                "ID|PATH",
                Value::JobId,
                "the number of a Job, or the path of a Queue or Group",
            ),
            optional(
                "KEY",
                Value::Free,
                "a setting by its option name without dashes",
            ),
        ],
        kind: Some("explain"),
        notes: &[
            "For a path, each setting is shown with the value configured there, the value in effect, the object that supplies it, and why it cannot take effect when an ancestor ceiling, a missing controller or an ancestor bound stands against it.",
        ],
        ..BASE
    },
    Command {
        path: &["logs"],
        section: Section::Inspection,
        summary: "print or follow the retained output of a Job",
        groups: &[group(
            "Options",
            &[
                short(flag("--follow", "keep printing until the Job ends"), "-f"),
                one(
                    "--stream",
                    "STREAM",
                    STREAMS,
                    "all, stdout, stderr, combined or diagnostic",
                ),
                one(
                    "--attempt",
                    "N",
                    Value::Free,
                    "the attempt instead of the current one",
                ),
                flag("--raw", "print bytes unchanged, terminal controls included"),
                flag("--json", "print one JSON object per record"),
                one(
                    "--since-ms",
                    "N",
                    Value::Free,
                    "only records from this Unix time in milliseconds",
                ),
                one(
                    "--until-ms",
                    "N",
                    Value::Free,
                    "only records up to this Unix time in milliseconds",
                ),
                one("--grep", "TEXT", Value::Free, "only lines containing TEXT"),
            ],
        )],
        operands: &[ANY_ID],
        notes: &[
            "--json prints a stream of records, one JSON object per line, and takes no --format json envelope.",
        ],
        ..BASE
    },
    Command {
        path: &["log"],
        section: Section::Inspection,
        summary: "query the recorded log of a Job",
        groups: &[group(
            "Options",
            &[one(
                "--attempt",
                "N",
                Value::Free,
                "the attempt instead of the current one",
            )],
        )],
        operands: &[
            ANY_ID,
            repeated(
                "QUERY",
                QUERIES,
                "errors, grep TEXT, lines A..B, tail N or full",
            ),
        ],
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["attempts"],
        section: Section::Inspection,
        summary: "list every execution attempt of a Job",
        groups: &[group("Options", &[JSON])],
        operands: &[ANY_ID],
        kind: Some("attempts"),
        ..BASE
    },
    Command {
        path: &["pressure"],
        section: Section::Inspection,
        summary: "show resource pressure of the host or of one Job",
        groups: &[group("Options", &[JSON])],
        operands: &[optional("ID", Value::JobId, "the number of a Job")],
        kind: Some("pressure"),
        notes: &["Pressure describes affected work, not the cause of contention."],
        ..BASE
    },
    Command {
        path: &["pressure", "status"],
        section: Section::Inspection,
        summary: "show the state of the pressure rules",
        groups: &[group("Options", &[JSON])],
        kind: Some("pressure_status"),
        ..BASE
    },
    Command {
        path: &["pressure", "events"],
        section: Section::Inspection,
        summary: "show recorded pressure events",
        groups: &[group("Options", &[JSON])],
        kind: Some("pressure_events"),
        ..BASE
    },
    Command {
        path: &["pressure", "replay"],
        section: Section::Inspection,
        summary: "evaluate pressure rules against a recorded file",
        groups: &[group("Options", &[JSON])],
        operands: &[required("FILE", Value::File, "a recorded pressure file")],
        kind: Some("pressure_replay"),
        ..BASE
    },
    Command {
        path: &["pressure", "parse"],
        section: Section::Inspection,
        summary: "read one kernel pressure file",
        groups: &[group(
            "Options",
            &[
                one(
                    "--resource",
                    "cpu|memory|io",
                    PRESSURES,
                    "the resource the file describes",
                ),
                flag("--host", "the file is the host-wide one"),
            ],
        )],
        operands: &[required("FILE", Value::File, "a kernel pressure file")],
        ..BASE
    },
    Command {
        path: &["host"],
        section: Section::Inspection,
        summary: "show the resources, devices and Queues of a host",
        groups: &[group(
            "Options",
            &[
                one(
                    "--on",
                    "USER@HOST",
                    Value::Free,
                    "ask the job service of another host over ssh",
                ),
                one(
                    "--ssh-key",
                    "FILE",
                    Value::File,
                    "the ssh key used with --on",
                ),
                many(
                    "--ssh-option",
                    "KEY=VALUE",
                    Value::Free,
                    "an ssh option used with --on",
                ),
                JSON,
            ],
        )],
        kind: Some("host"),
        ..BASE
    },
    Command {
        path: &["metrics"],
        section: Section::Inspection,
        summary: "print the service's metrics in the Prometheus text format",
        notes: &[
            "When the service does not answer, job_up 0 is printed and the exit status is 125.",
            "The service can also serve them over HTTP; see the [metrics] table in job.conf(5).",
        ],
        ..BASE
    },
    Command {
        path: &["screenshot"],
        section: Section::Inspection,
        summary: "write a PNG of a monitor or of the window of a Job",
        groups: &[group(
            "Options",
            &[
                short(
                    one("--output", "FILE", Value::File, "the file to write"),
                    "-o",
                ),
                one(
                    "--job",
                    "ID",
                    Value::JobId,
                    "capture the window of this Job",
                ),
                one(
                    "--pid",
                    "PID",
                    Value::Free,
                    "capture the window of this process and its children",
                ),
                one("--display", "NAME", Value::Free, "the X display to use"),
            ],
        )],
        operands: &[optional(
            "MONITOR",
            Value::Free,
            "a monitor by name, left, right, primary or number",
        )],
        ..BASE
    },
    Command {
        path: &["net"],
        section: Section::Inspection,
        summary: "show the network namespaces and profiles the service configuration names",
        ..BASE
    },
    Command {
        path: &["net", "list"],
        section: Section::Inspection,
        summary: "list the configured network namespaces and profiles",
        groups: &[group("Options", &[JSON])],
        ..BASE
    },
    Command {
        path: &["net", "show"],
        section: Section::Inspection,
        summary: "show one network namespace or profile with its rules",
        groups: &[group("Options", &[JSON])],
        operands: &[required(
            "NAME",
            Value::Free,
            "a name from the service configuration, or ns:NAME or profile:NAME",
        )],
        ..BASE
    },
    Command {
        path: &["config"],
        section: Section::Administration,
        summary: "check, create, show or reload the service configuration",
        ..BASE
    },
    Command {
        path: &["config", "check"],
        section: Section::Administration,
        summary: "validate a configuration file",
        operands: &[required("FILE", Value::File, "a configuration file")],
        ..BASE
    },
    Command {
        path: &["config", "init"],
        section: Section::Administration,
        summary: "create a configuration file",
        groups: &[group(
            "Options",
            &[one(
                "--profile",
                "ordinary|legacy",
                PROFILES,
                "the service profile",
            )],
        )],
        operands: &[required("FILE", Value::File, "a configuration file")],
        ..BASE
    },
    Command {
        path: &["config", "show"],
        section: Section::Administration,
        summary: "show the effective service policy and its source",
        groups: &[group("Options", &[JSON])],
        kind: Some("config_show"),
        ..BASE
    },
    Command {
        path: &["config", "reload"],
        section: Section::Administration,
        summary: "reload the same profile while the service is drained",
        ..BASE
    },
    Command {
        path: &["state"],
        section: Section::Administration,
        summary: "validate, back up, migrate or restore offline state",
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["state", "validate"],
        section: Section::Administration,
        summary: "validate offline state and report migration readiness",
        groups: &[group(
            "Options",
            &[one(
                "--source",
                "PATH",
                Value::Dir,
                "the state directory to read",
            )],
        )],
        ..BASE
    },
    Command {
        path: &["state", "backup"],
        section: Section::Administration,
        summary: "create a verified backup of offline state",
        groups: &[group(
            "Options",
            &[
                one(
                    "--source",
                    "PATH",
                    Value::Dir,
                    "the state directory to read",
                ),
                one(
                    "--destination",
                    "PATH",
                    Value::Dir,
                    "the new directory to write",
                ),
                flag(
                    "--interrupted",
                    "also copy records of Jobs that were executing when their processes ended; the restored service records them as lost",
                ),
            ],
        )],
        ..BASE
    },
    Command {
        path: &["state", "migrate"],
        section: Section::Administration,
        summary: "migrate offline state into a new directory",
        groups: &[group(
            "Options",
            &[
                one(
                    "--source",
                    "PATH",
                    Value::Dir,
                    "the state directory to read",
                ),
                one(
                    "--destination",
                    "PATH",
                    Value::Dir,
                    "the new directory to write",
                ),
                one(
                    "--backup",
                    "PATH",
                    Value::Dir,
                    "where the verified backup goes",
                ),
                one(
                    "--profile",
                    "ordinary|legacy",
                    PROFILES,
                    "the service profile",
                ),
                DRY_RUN,
            ],
        )],
        ..BASE
    },
    Command {
        path: &["state", "restore"],
        section: Section::Administration,
        summary: "restore a backup into a new directory",
        groups: &[group(
            "Options",
            &[
                one(
                    "--source",
                    "PATH",
                    Value::Dir,
                    "the state directory to read",
                ),
                one(
                    "--destination",
                    "PATH",
                    Value::Dir,
                    "the new directory to write",
                ),
            ],
        )],
        ..BASE
    },
    Command {
        path: &["doctor"],
        section: Section::Administration,
        summary: "check the installation and say what is missing",
        groups: &[group("Options", &[JSON, SYSTEM])],
        kind: Some("doctor"),
        notes: &["Exit status 1 means that a check failed; the report is printed either way."],
        ..BASE
    },
    Command {
        path: &["audit"],
        section: Section::Administration,
        summary: "read the record of requests that changed something",
        groups: &[group(
            "Options",
            &[
                one(
                    "--target",
                    "ID|PATH",
                    Value::Free,
                    "only records about this Job, Queue or Group",
                ),
                one(
                    "--action",
                    "NAME",
                    Value::Free,
                    "only records of this action",
                ),
                one(
                    "--since",
                    "MS",
                    Value::Free,
                    "only records from this Unix time in milliseconds on",
                ),
                JSON,
            ],
        )],
        kind: Some("audit"),
        notes: &[
            "--json prints one JSON object per record; --format json prints one envelope whose data holds schema_version, entries and unreadable.",
        ],
        ..BASE
    },
    Command {
        path: &["events"],
        section: Section::Administration,
        summary: "read or follow the record of state changes of Jobs, Queues and Groups",
        groups: &[group(
            "Options",
            &[
                short(
                    flag("--follow", "keep printing new records until interrupted"),
                    "-f",
                ),
                one("--job", "ID", Value::JobId, "only records about this Job"),
                one(
                    "--queue",
                    "PATH",
                    Value::QueuePath,
                    "only records about this Queue or Group and what lies below it",
                ),
                one(
                    "--since",
                    "MS",
                    Value::Free,
                    "only records from this Unix time in milliseconds on",
                ),
                one(
                    "--format",
                    "FORMAT",
                    FORMATS,
                    "text, or json for one JSON object per line",
                ),
                JSON,
            ],
        )],
        notes: &[
            "Each record is one transition; a Job put back into its Queue is written as requeued.",
        ],
        ..BASE
    },
    Command {
        path: &["completion"],
        section: Section::Administration,
        summary: "print a completion script for a shell",
        operands: &[required("SHELL", SHELLS, "bash, fish or zsh")],
        ..BASE
    },
    Command {
        path: &["help"],
        section: Section::Administration,
        summary: "show help for job or for one command",
        groups: &[group(
            "Options",
            &[
                flag("--all", "print the long text of every part"),
                flag("--syntax", "print the syntax reference as Markdown"),
            ],
        )],
        operands: &[repeated("COMMAND", Value::Free, "a command of job")],
        ..BASE
    },
    Command {
        path: &["daemon"],
        section: Section::Internal,
        summary: "run the service for this host",
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["hook"],
        section: Section::Internal,
        summary: "answer one JSON question about a command line: allow, deny or rewrite",
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["classify"],
        section: Section::Internal,
        summary: "print the verdict of the hook for each command line on standard input",
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["policy"],
        section: Section::Internal,
        summary: "print the verdict of the command policy for each line on standard input",
        groups: &[group(
            "Options",
            &[flag(
                "--show",
                "print the policy file in use and its rules, and read no commands",
            )],
        )],
        operands: &[optional("NAME", Value::Free, "the name of the caller")],
        check: Check::None,
        ..BASE
    },
    Command {
        path: &["shim"],
        section: Section::Internal,
        summary: "supervise one Job for the service",
        check: Check::None,
        hidden: true,
        ..BASE
    },
    Command {
        path: &["remote"],
        section: Section::Internal,
        summary: "serve a request of another host over ssh",
        check: Check::None,
        hidden: true,
        ..BASE
    },
    Command {
        path: &["link-wireguard"],
        section: Section::Internal,
        summary: "configure a WireGuard interface inside a namespace",
        check: Check::None,
        hidden: true,
        ..BASE
    },
    Command {
        path: &["__complete"],
        section: Section::Internal,
        summary: "print completion candidates for a shell",
        operands: &[
            required("KIND", KINDS, "what to complete"),
            repeated("WORD", Value::Free, "the word being completed"),
        ],
        check: Check::None,
        exits: SILENT,
        hidden: true,
        ..BASE
    },
];

macro_rules! objects {
    ($name:ident, $kind:literal, $path:expr, $set:expr, $($extra:expr),* $(,)?) => {
        pub static $name: &[Command] = &[
            Command {
                path: &[$kind, "create"],
                summary: "create an object without settings of its own",
                groups: CREATE,
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_create")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "list"],
                summary: "list the objects of this kind",
                groups: &[group("Options", &[LABEL_FILTER, JSON])],
                notes: &[
                    "The tsv columns are id, path, kind, paused, closed and labels; labels is JSON.",
                ],
                kind: Some(concat!($kind, "_list")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "show"],
                summary: "show an object with its effective configuration",
                groups: &[group("Options", &[JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_show")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "set"],
                summary: "set local defaults and constraints",
                groups: $set,
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_set")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "unset"],
                summary: "remove local settings so that inheritance applies again",
                groups: &[group("Options", &[JSON])],
                operands: &[
                    required("PATH", $path, "the path of a Queue or Group"),
                    repeated("KEY", Value::Free, "a setting by its option name without dashes"),
                ],
                notes: &["Write label-KEY to remove one label and labels to remove all of them."],
                kind: Some(concat!($kind, "_unset")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "rename"],
                summary: "rename an object without changing its identity",
                groups: &[group("Options", &[JSON])],
                operands: &[
                    required("PATH", $path, "the path of a Queue or Group"),
                    required("NAME", Value::Free, "the new name"),
                ],
                kind: Some(concat!($kind, "_rename")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "move"],
                summary: "move an inactive object into another Group",
                groups: &[group("Options", &[PARENT, JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_move")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "close"],
                summary: "refuse new submissions",
                groups: &[group("Options", &[JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_close")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "open"],
                summary: "accept new submissions again",
                groups: &[group("Options", &[JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                kind: Some(concat!($kind, "_open")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "update"],
                summary: "change resource controls of an existing subtree",
                groups: UPDATE,
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                exits: CONTROL,
                kind: Some(concat!($kind, "_update")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "suspend"],
                summary: "freeze every running Job below the path",
                groups: &[group("Options", &[RECURSIVE, TIMEOUT, JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                check: Check::Dispatch,
                exits: CONTROL,
                kind: Some(concat!($kind, "_suspend")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "continue"],
                summary: "let every suspended Job below the path run again",
                groups: &[group("Options", &[RECURSIVE, TIMEOUT, JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                check: Check::Dispatch,
                exits: CONTROL,
                kind: Some(concat!($kind, "_continue")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "cancel"],
                summary: "cancel every Job below the path",
                groups: &[group("Options", &[RECURSIVE, DRY_RUN, JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                check: Check::Dispatch,
                exits: CONTROL,
                kind: Some(concat!($kind, "_cancel")),
                ..OBJECT
            },
            Command {
                path: &[$kind, "remove"],
                summary: "remove an object and the records of its completed Jobs",
                groups: &[group("Options", &[RECURSIVE, DRY_RUN, ALLOW_LOST, JSON])],
                operands: &[required("PATH", $path, "the path of a Queue or Group")],
                check: Check::Dispatch,
                exits: CONTROL,
                kind: Some(concat!($kind, "_remove")),
                ..OBJECT
            },
            $($extra),*
        ];
    };
}

const OBJECT: Command = Command {
    section: Section::Organization,
    check: Check::Parser,
    ..BASE
};

const PARENT: Opt = one(
    "--group",
    "PATH",
    Value::GroupPath,
    "the Group that contains the object",
);

const CREATE: &[Group] = &[
    group("Options", &[PARENT]),
    SETTINGS[0],
    SETTINGS[1],
    SETTINGS[2],
    SETTINGS[3],
    SETTINGS[4],
    SETTINGS[5],
    SETTINGS[6],
];

const QUEUE_SET: &[Group] = &[
    SETTINGS[0],
    SETTINGS[1],
    SETTINGS[2],
    SETTINGS[3],
    SETTINGS[4],
    SETTINGS[5],
    SETTINGS[6],
    EARLIER,
];

const NAME: Operand = required("NAME", Value::QueuePath, "the name of a Queue");

objects!(
    QUEUE,
    "queue",
    Value::QueuePath,
    QUEUE_SET,
    Command {
        path: &["queue"],
        summary: "organize Queues; without a subcommand, show every Queue and its Jobs",
        operands: &[optional("NAME", Value::QueuePath, "the name of a Queue")],
        check: Check::None,
        ..OBJECT
    },
    Command {
        path: &["queue", "pause"],
        summary: "hold the waiting Jobs of a Queue",
        groups: &[group("Options", &[JSON])],
        operands: &[NAME],
        check: Check::Dispatch,
        kind: Some("queue_pause"),
        ..OBJECT
    },
    Command {
        path: &["queue", "resume"],
        summary: "let the waiting Jobs of a Queue start again",
        groups: &[group("Options", &[JSON])],
        operands: &[NAME],
        check: Check::Dispatch,
        kind: Some("queue_resume"),
        ..OBJECT
    },
    Command {
        path: &["queue", "add"],
        section: Section::Legacy,
        summary: "earlier spelling of queue create",
        groups: &[EARLIER],
        operands: &[NAME],
        check: Check::None,
        ..OBJECT
    },
    Command {
        path: &["queue", "clear"],
        section: Section::Legacy,
        summary: "cancel every Job waiting in a Queue",
        operands: &[NAME],
        check: Check::None,
        ..OBJECT
    },
    Command {
        path: &["queue", "rm"],
        section: Section::Legacy,
        summary: "earlier spelling of queue remove",
        groups: &[Group {
            heading: "Earlier Queue settings",
            legacy: true,
            options: &[flag(
                "--when-empty",
                "take no new Jobs and remove the Queue once it is empty",
            )],
        }],
        operands: &[NAME],
        check: Check::None,
        ..OBJECT
    },
);

objects!(
    GROUP,
    "group",
    Value::GroupPath,
    SETTINGS,
    Command {
        path: &["group"],
        summary: "organize Groups of Groups and Queues",
        ..OBJECT
    },
    Command {
        path: &["group", "pause"],
        summary: "hold the waiting Jobs below a Group",
        groups: &[group("Options", &[JSON])],
        operands: &[required(
            "PATH",
            Value::GroupPath,
            "the path of a Queue or Group"
        )],
        kind: Some("group_pause"),
        ..OBJECT
    },
    Command {
        path: &["group", "resume"],
        summary: "let the waiting Jobs below a Group start again",
        groups: &[group("Options", &[JSON])],
        operands: &[required(
            "PATH",
            Value::GroupPath,
            "the path of a Queue or Group"
        )],
        kind: Some("group_resume"),
        ..OBJECT
    },
);

pub static PRESETS: &[Command] = &[
    Command {
        path: &["profile"],
        section: Section::Organization,
        summary: "list or show versioned execution profiles",
        groups: &[group("Options", &[JSON])],
        kind: Some("profile_list"),
        ..BASE
    },
    Command {
        path: &["profile", "list"],
        section: Section::Organization,
        summary: "list every execution profile revision",
        groups: &[group("Options", &[JSON])],
        kind: Some("profile_list"),
        ..BASE
    },
    Command {
        path: &["profile", "show"],
        section: Section::Organization,
        summary: "show one execution profile revision",
        groups: &[group("Options", &[JSON])],
        operands: &[required("NAME@REV", Value::Preset, "a preset revision")],
        kind: Some("profile_show"),
        ..BASE
    },
    Command {
        path: &["class"],
        section: Section::Organization,
        summary: "list or show versioned scheduling classes",
        groups: &[group("Options", &[JSON])],
        kind: Some("class_list"),
        ..BASE
    },
    Command {
        path: &["class", "list"],
        section: Section::Organization,
        summary: "list every scheduling class revision",
        groups: &[group("Options", &[JSON])],
        kind: Some("class_list"),
        ..BASE
    },
    Command {
        path: &["class", "show"],
        section: Section::Organization,
        summary: "show one scheduling class revision",
        groups: &[group("Options", &[JSON])],
        operands: &[required("NAME@REV", Value::Preset, "a preset revision")],
        kind: Some("class_show"),
        ..BASE
    },
];
