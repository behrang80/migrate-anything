//! Operator consent: proof that a human at an interactive terminal confirmed an action.
//!
//! `OperatorConsent` has a private field, so it can only be created by
//! [`OperatorConsent::from_terminal_challenge`] (or the test-only constructor). The MCP server
//! never calls either: agent-facing code paths cannot approve anything.

use std::io::{BufRead, IsTerminal, Write};

use ma_types::{MaError, MaResult};

#[derive(Debug, Clone)]
pub struct OperatorConsent {
    operator: String,
    method: &'static str,
}

impl OperatorConsent {
    pub fn operator(&self) -> &str {
        &self.operator
    }
    pub fn method(&self) -> &'static str {
        self.method
    }

    /// Show `summary` on the terminal and require the operator to type a random challenge code.
    /// Refuses when stdin/stderr are not terminals (e.g. when driven by an agent's tool call).
    pub fn from_terminal_challenge(summary: &str) -> MaResult<OperatorConsent> {
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(MaError::policy(
                "approval requires an interactive terminal operated by a human; it cannot be granted through pipes, scripts or AI tool calls",
            ));
        }
        let code = ma_security::random_hex(3).to_uppercase();
        let mut err = std::io::stderr();
        writeln!(err, "{summary}").ok();
        writeln!(err, "\nTo approve, type the confirmation code {code} and press Enter (anything else aborts):").ok();
        write!(err, "> ").ok();
        err.flush().ok();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| MaError::policy(format!("could not read confirmation: {e}")))?;
        if line.trim() != code {
            return Err(MaError::policy("confirmation code did not match; nothing was approved"));
        }
        Ok(OperatorConsent { operator: ma_security::os_operator(), method: "interactive-tty-challenge" })
    }

    /// Test-only consent. Compiled only with the `test-approval` feature and additionally
    /// requires `MA_INSECURE_TEST_APPROVAL=1` at runtime.
    #[cfg(feature = "test-approval")]
    pub fn for_tests(operator: &str) -> MaResult<OperatorConsent> {
        if std::env::var("MA_INSECURE_TEST_APPROVAL").as_deref() != Ok("1") {
            return Err(MaError::policy("test approval is disabled"));
        }
        Ok(OperatorConsent { operator: operator.to_string(), method: "test-harness" })
    }
}
