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

/// Proof that this process was started by a human from an interactive terminal.
///
/// The local web UI ("Studio") uses it for approvals: the server prints a one-time code on the
/// terminal it was started from and the operator types that code into the browser. An agent that
/// drives only the browser cannot see the terminal, and an agent that starts the UI from a
/// non-interactive shell does not get a session at all, so approvals stay with the human.
#[derive(Debug)]
pub struct InteractiveSession {
    _private: (),
}

/// How long a printed approval code stays valid.
const CHALLENGE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const CHALLENGE_ATTEMPTS: u8 = 5;

impl InteractiveSession {
    /// Succeeds only when both stdin and stderr are terminals.
    pub fn from_terminal() -> MaResult<InteractiveSession> {
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(MaError::policy("approvals need a process started from an interactive terminal; this one was started without one"));
        }
        Ok(InteractiveSession { _private: () })
    }

    /// Test-only session (see [`OperatorConsent::for_tests`]).
    #[cfg(feature = "test-approval")]
    pub fn for_tests() -> MaResult<InteractiveSession> {
        if std::env::var("MA_INSECURE_TEST_APPROVAL").as_deref() != Ok("1") {
            return Err(MaError::policy("test approval is disabled"));
        }
        Ok(InteractiveSession { _private: () })
    }

    /// Print a fresh one-time code on the terminal for `what` and return the pending challenge.
    pub fn challenge(&self, what: &str) -> Challenge {
        let code = ma_security::random_hex(3).to_uppercase();
        let mut err = std::io::stderr();
        let line = "=".repeat(64);
        writeln!(err, "\n{line}\n  APPROVAL REQUESTED in Migrate Anything Studio\n  {what}\n\n  Type this code in the browser to approve:   {code}\n  (valid for 5 minutes; ignore it if you did not ask for this)\n{line}").ok();
        err.flush().ok();
        Challenge { code, created: std::time::Instant::now(), attempts_left: CHALLENGE_ATTEMPTS }
    }
}

/// A one-time approval code that was shown on the operator's terminal.
#[derive(Debug)]
pub struct Challenge {
    code: String,
    created: std::time::Instant,
    attempts_left: u8,
}

impl Challenge {
    pub fn is_expired(&self) -> bool {
        self.created.elapsed() > CHALLENGE_TTL || self.attempts_left == 0
    }

    /// Check the code the operator typed. A wrong code uses up one of the attempts.
    pub fn confirm(&mut self, typed: &str) -> MaResult<OperatorConsent> {
        if self.is_expired() {
            return Err(MaError::policy("this approval code has expired; request a new one"));
        }
        let typed: String = typed.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
        if typed != self.code {
            self.attempts_left -= 1;
            return Err(MaError::policy(format!(
                "that code does not match the one shown in the terminal ({} attempt(s) left)",
                self.attempts_left
            )));
        }
        self.attempts_left = 0; // single use
        Ok(OperatorConsent { operator: ma_security::os_operator(), method: "terminal-code-via-local-ui" })
    }

    /// Test-only access to the code that was "shown on the terminal".
    #[cfg(feature = "test-approval")]
    pub fn code_for_tests(&self) -> &str {
        &self.code
    }
}
