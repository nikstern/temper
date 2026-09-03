use temper_runtime::persistence::{
    CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET, CreateOrVerifyStoreOutcome, PersistenceError,
};

use super::{redis_acknowledgement_unknown, redis_post_commit, redis_pre_commit};

fn parse_result_flag(value: &str, committed: bool) -> Result<bool, PersistenceError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ if committed => Err(redis_post_commit(format!(
            "invalid committed Redis boolean flag: {value:?}"
        ))),
        _ => Err(redis_pre_commit(format!(
            "invalid rejected Redis boolean flag: {value:?}"
        ))),
    }
}

fn parse_creation_sequence(value: &str) -> Result<u64, PersistenceError> {
    let sequence = value.parse::<u64>().map_err(redis_post_commit)?;
    if sequence != 1 {
        return Err(redis_post_commit(format!(
            "create-or-verify returned non-creation sequence {sequence}"
        )));
    }
    Ok(sequence)
}

fn parse_conflict_fields(value: &str) -> Result<Vec<String>, PersistenceError> {
    let fields: Vec<String> = serde_json::from_str(value).map_err(redis_pre_commit)?;
    if fields.len() > CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET {
        return Err(redis_pre_commit(format!(
            "create-or-verify returned {} conflict fields, budget is {CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET}",
            fields.len()
        )));
    }
    if fields.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(redis_pre_commit(
            "create-or-verify conflict fields are not sorted and unique",
        ));
    }
    Ok(fields)
}

/// Decode one string-tagged atomic create-or-verify result without losing commit evidence.
pub(super) fn decode_create_or_verify_result(
    result: &[String],
) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
    let Some(status) = result.first().map(String::as_str) else {
        return Err(redis_acknowledgement_unknown(
            "empty create-or-verify Lua result",
        ));
    };
    match status {
        "created" => {
            let [_, entity_id, sequence] = result else {
                return Err(redis_post_commit(format!(
                    "malformed created Lua result: {result:?}"
                )));
            };
            Ok(CreateOrVerifyStoreOutcome::Created {
                entity_id: entity_id.clone(),
                sequence_nr: parse_creation_sequence(sequence)?,
            })
        }
        "already_matches" => {
            let [_, entity_id, sequence, pending] = result else {
                return Err(redis_post_commit(format!(
                    "malformed already_matches Lua result: {result:?}"
                )));
            };
            Ok(CreateOrVerifyStoreOutcome::AlreadyMatches {
                entity_id: entity_id.clone(),
                sequence_nr: parse_creation_sequence(sequence)?,
                notification_pending: parse_result_flag(pending, true)?,
            })
        }
        "conflict" => {
            let [_, fields, truncated] = result else {
                return Err(redis_pre_commit(format!(
                    "malformed conflict Lua result: {result:?}"
                )));
            };
            Ok(CreateOrVerifyStoreOutcome::Conflict {
                fields: parse_conflict_fields(fields)?,
                truncated: parse_result_flag(truncated, false)?,
            })
        }
        "migration_required" if result.len() == 1 => {
            Ok(CreateOrVerifyStoreOutcome::CreationContractMigrationRequired)
        }
        "migration_required" => Err(redis_pre_commit(format!(
            "malformed migration_required Lua result: {result:?}"
        ))),
        "publication_fenced" => Err(redis_pre_commit(format!(
            "stream descriptor publication fence: {result:?}"
        ))),
        _ => Err(redis_acknowledgement_unknown(format!(
            "unexpected create-or-verify Lua result: {result:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn known_create_or_verify_tags_retain_causal_phase_when_malformed() {
        for result in [
            strings(&["created"]),
            strings(&["created", "id", "bad"]),
            strings(&["already_matches"]),
            strings(&["already_matches", "id", "1", "bad-flag"]),
        ] {
            assert!(matches!(
                decode_create_or_verify_result(&result),
                Err(PersistenceError::PostCommit(_))
            ));
        }
        for result in [
            strings(&["conflict"]),
            strings(&["conflict", "not-json", "0"]),
            strings(&["conflict", "[]", "bad-flag"]),
            strings(&["migration_required", "extra"]),
            strings(&["publication_fenced", "extra"]),
        ] {
            assert!(matches!(
                decode_create_or_verify_result(&result),
                Err(PersistenceError::PreCommit(_))
            ));
        }
        for result in [Vec::new(), strings(&["future_status"])] {
            assert!(matches!(
                decode_create_or_verify_result(&result),
                Err(PersistenceError::AcknowledgementUnknown(_))
            ));
        }
    }

    #[test]
    fn valid_boolean_flags_are_not_coerced() {
        let pending =
            decode_create_or_verify_result(&strings(&["already_matches", "entity", "1", "1"]))
                .expect("valid committed result");
        assert!(matches!(
            pending,
            CreateOrVerifyStoreOutcome::AlreadyMatches {
                notification_pending: true,
                ..
            }
        ));
        let conflict = decode_create_or_verify_result(&strings(&["conflict", "[]", "0"]))
            .expect("valid rejected result");
        assert!(matches!(
            conflict,
            CreateOrVerifyStoreOutcome::Conflict {
                truncated: false,
                ..
            }
        ));
    }

    #[test]
    fn creation_sequences_are_exactly_one() {
        for status in ["created", "already_matches"] {
            for sequence in ["0", "2"] {
                let result = if status == "created" {
                    strings(&[status, "entity", sequence])
                } else {
                    strings(&[status, "entity", sequence, "0"])
                };
                assert!(matches!(
                    decode_create_or_verify_result(&result),
                    Err(PersistenceError::PostCommit(_))
                ));
            }
        }
    }

    #[test]
    fn conflict_fields_are_bounded_sorted_and_unique() {
        let oversized = (0..=CREATE_OR_VERIFY_CONFLICT_FIELD_BUDGET)
            .map(|index| format!("field_{index:02}"))
            .collect::<Vec<_>>();
        let cases = [
            serde_json::to_string(&oversized).expect("conflict fields"),
            serde_json::to_string(&["a", "a"]).expect("duplicate fields"),
            serde_json::to_string(&["b", "a"]).expect("unordered fields"),
        ];
        for fields in cases {
            assert!(matches!(
                decode_create_or_verify_result(&strings(&["conflict", &fields, "0"])),
                Err(PersistenceError::PreCommit(_))
            ));
        }
    }
}
