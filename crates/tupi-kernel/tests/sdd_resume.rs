//! Contrato das variantes de retomada de um plano SDD persistido.

use tupi_domain::AgentMode;
use tupi_kernel::{Command, CommandOutcome};

#[test]
fn resume_sdd_carries_the_slug() {
    let command = Command::ResumeSdd {
        slug: "auth".into(),
    };

    match command {
        Command::ResumeSdd { slug } => assert_eq!(slug, "auth"),
        _ => panic!("esperava Command::ResumeSdd"),
    }
}

#[test]
fn sdd_resumed_reports_slug_title_mode_and_open_tasks() {
    let outcome = CommandOutcome::SddResumed {
        slug: "auth".into(),
        title: "Autenticação".into(),
        mode: AgentMode::Sdd,
        open_tasks: 2,
    };

    match outcome {
        CommandOutcome::SddResumed {
            slug,
            title,
            mode,
            open_tasks,
        } => {
            assert_eq!(slug, "auth");
            assert_eq!(title, "Autenticação");
            assert_eq!(mode.as_u8(), AgentMode::Sdd.as_u8());
            assert_eq!(open_tasks, 2);
        }
        _ => panic!("esperava CommandOutcome::SddResumed"),
    }
}
