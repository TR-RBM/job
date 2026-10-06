use std::process::ExitCode;

pub fn without_json(args: &[String]) -> (Vec<String>, bool) {
    (
        args.iter()
            .filter(|word| word.as_str() != "--json")
            .cloned()
            .collect(),
        args.iter().any(|word| word == "--json"),
    )
}

pub fn leading_id(args: &[String], valued: &[&str]) -> (Option<String>, Vec<String>) {
    let mut index = 0;
    while index < args.len() {
        let word = args[index].as_str();
        if valued.contains(&word) {
            index += 2;
        } else if word.starts_with('-') && word.len() > 1 {
            index += 1;
        } else {
            let mut others = args.to_vec();
            return (Some(others.remove(index)), others);
        }
    }
    (None, args.to_vec())
}

pub fn record(job: &crate::model::Job) -> Result<ExitCode, String> {
    println!(
        "{}",
        serde_json::to_string_pretty(job).map_err(|error| error.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}
