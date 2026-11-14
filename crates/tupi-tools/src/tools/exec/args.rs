use super::*;

pub(crate) fn build_args(params: &ExecParams) -> Result<Vec<String>, ToolError> {
    let mut args: Vec<String> = Vec::new();

    if params.kind == ExecKind::Xtask {
        let task = params
            .task
            .as_deref()
            .map(str::trim)
            .filter(|task| !task.is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArguments("task e obrigatorio quando kind e xtask".to_string())
            })?;
        args.push("xtask".to_string());
        args.push(task.to_string());
        return Ok(args);
    }
    if params.task.is_some() {
        return Err(ToolError::InvalidArguments(
            "task so vale para kind xtask".to_string(),
        ));
    }

    args.push(
        match params.kind {
            ExecKind::Build => "build",
            ExecKind::Check => "check",
            ExecKind::Test => "test",
            ExecKind::Clippy => "clippy",
            ExecKind::Xtask => unreachable!(),
        }
        .to_string(),
    );

    match params
        .package
        .as_deref()
        .map(str::trim)
        .filter(|package| !package.is_empty())
    {
        Some(package) => {
            args.push("-p".to_string());
            args.push(package.to_string());
        }
        None => args.push("--workspace".to_string()),
    }

    if let Some(target) = params
        .target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    {
        match target {
            "lib" => args.push("--lib".to_string()),
            "bins" => args.push("--bins".to_string()),
            "tests" => args.push("--tests".to_string()),
            "all-targets" => args.push("--all-targets".to_string()),
            "doc" => {
                if params.kind != ExecKind::Test {
                    return Err(ToolError::InvalidArguments(
                        "target doc so vale para kind test".to_string(),
                    ));
                }
                args.push("--doc".to_string());
            }
            name => {
                if params.kind != ExecKind::Test {
                    return Err(ToolError::InvalidArguments(format!(
                        "target {name} so vale para kind test; use lib, bins, tests ou all-targets"
                    )));
                }
                args.push("--test".to_string());
                args.push(name.to_string());
            }
        }
    }

    match params
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|filter| !filter.is_empty())
    {
        Some(filter) => {
            if params.kind != ExecKind::Test {
                return Err(ToolError::InvalidArguments(
                    "filter so vale para kind test".to_string(),
                ));
            }
            args.push("--".to_string());
            args.push(filter.to_string());
            if params.exact {
                args.push("--exact".to_string());
            }
        }
        None if params.exact => {
            return Err(ToolError::InvalidArguments(
                "exact exige filter".to_string(),
            ));
        }
        None => {}
    }

    Ok(args)
}

pub(crate) fn command_line(args: &[String]) -> String {
    format!("cargo {}", args.join(" "))
}

pub(crate) fn execute(
    root: &Path,
    params: &ExecParams,
    timeout: Duration,
) -> Result<Output, ToolError> {
    let args = build_args(params)?;
    proc::cargo(&args, root, timeout)
}

pub(crate) fn timeout_for(params: &ExecParams) -> Duration {
    Duration::from_secs(
        params
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS),
    )
}
