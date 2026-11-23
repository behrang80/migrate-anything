//! Approval signing and verification (HMAC-SHA256 over canonical JSON claims).

use hmac::{Hmac, KeyInit, Mac};
use ma_types::{ApprovalClaims, ApprovalRecord, MaError, MaResult, MigrationPlan, Operation};
use sha2::Sha256;

use crate::keys::Key;

type HmacSha256 = Hmac<Sha256>;

fn mac_for(key: &Key, claims: &ApprovalClaims) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(&key.0).expect("hmac accepts any key length");
    mac.update(ma_types::hash::canonical_json(claims).as_bytes());
    mac
}

pub fn sign(key: &Key, claims: &ApprovalClaims) -> String {
    hex::encode(mac_for(key, claims).finalize().into_bytes())
}

/// Verify that an approval is authentic, unexpired, not revoked, and bound to exactly this plan
/// and these operations. Returns a precise reason on failure.
pub fn verify(key: &Key, record: &ApprovalRecord, plan: &MigrationPlan, plan_hash: &str, requested_ops: &[Operation]) -> MaResult<()> {
    let given = hex::decode(&record.signature).map_err(|_| MaError::policy("approval signature is malformed"))?;
    // constant-time comparison
    if mac_for(key, &record.claims).verify_slice(&given).is_err() {
        return Err(MaError::policy("approval signature is invalid"));
    }
    if record.revoked {
        return Err(MaError::policy("approval has been revoked"));
    }
    let c = &record.claims;
    if chrono::Utc::now() > c.expires_at {
        return Err(MaError::policy("approval has expired"));
    }
    if c.plan_id != plan.plan_id || c.plan_hash != plan_hash || plan.compute_hash() != plan_hash {
        return Err(MaError::policy("approval does not match this plan (plan changed since approval)"));
    }
    if c.workspace != plan.workspace {
        return Err(MaError::policy("approval belongs to a different workspace"));
    }
    if c.source_identity != plan.source.identity || c.destination_identity != plan.destination.identity {
        return Err(MaError::policy("approval is bound to different systems"));
    }
    if c.source_connector != plan.source.connector || c.destination_connector != plan.destination.connector {
        return Err(MaError::policy("approval is bound to different connector versions"));
    }
    if c.target_environment != plan.destination.environment {
        return Err(MaError::policy("approval is bound to a different target environment"));
    }
    for op in requested_ops {
        if !c.permitted_operations.contains(op) {
            return Err(MaError::policy(format!("operation {op:?} is not permitted by the approval")));
        }
        if op.is_destructive() && !c.destructive_authorized {
            return Err(MaError::policy(format!("destructive operation {op:?} requires separate authorization")));
        }
    }
    Ok(())
}
