//! Production-local authority for native household mutations.

use std::fmt;

use heyfood_application::{
    HouseholdMutationAuthorityPort, HouseholdMutationAuthorityV1, HouseholdMutationPurposeV1,
    PortError,
};
use heyfood_core::{CanonicalDateV1, CanonicalTimestampV1, CommitId, MemberId};
use time::OffsetDateTime;
use uuid::Uuid;

/// Stateless OS-backed allocator retained by one account/mode-bound household
/// session. UUID generation uses the platform CSPRNG and time is frozen once
/// per allocation.
pub struct NativeHouseholdMutationAuthorityV1;

impl NativeHouseholdMutationAuthorityV1 {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for NativeHouseholdMutationAuthorityV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NativeHouseholdMutationAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeHouseholdMutationAuthorityV1")
            .finish_non_exhaustive()
    }
}

impl HouseholdMutationAuthorityPort for NativeHouseholdMutationAuthorityV1 {
    fn allocate(
        &self,
        purpose: HouseholdMutationPurposeV1,
    ) -> Result<HouseholdMutationAuthorityV1, PortError> {
        let now = OffsetDateTime::now_utc();
        let frozen_commit_timestamp =
            CanonicalTimestampV1::from_datetime(now).map_err(authority_error)?;
        let frozen_evaluation_date = CanonicalDateV1::parse(format!(
            "{:04}-{:02}-{:02}",
            now.year(),
            u8::from(now.month()),
            now.day()
        ))
        .map_err(authority_error)?;
        let member_id = if purpose == HouseholdMutationPurposeV1::CreateMember {
            Some(MemberId::from_native_uuid_v4(Uuid::new_v4()).map_err(authority_error)?)
        } else {
            None
        };
        Ok(HouseholdMutationAuthorityV1 {
            commit_id: CommitId::from_uuid(Uuid::new_v4()),
            frozen_commit_timestamp,
            frozen_evaluation_date,
            member_id,
        })
    }
}

fn authority_error(_error: heyfood_core::HouseholdStateError) -> PortError {
    PortError::new(
        "household_mutation_authority_unavailable",
        "native household mutation authority is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_authority_allocates_exact_purpose_shapes_and_redacts_debug() {
        let authority = NativeHouseholdMutationAuthorityV1::new();
        let create = authority
            .allocate(HouseholdMutationPurposeV1::CreateMember)
            .unwrap();
        let save = authority
            .allocate(HouseholdMutationPurposeV1::SaveMemberProfile)
            .unwrap();
        let select = authority
            .allocate(HouseholdMutationPurposeV1::SelectScope)
            .unwrap();

        assert!(
            create
                .member_id
                .as_ref()
                .is_some_and(MemberId::is_native_uuid_v4)
        );
        assert!(save.member_id.is_none());
        assert!(select.member_id.is_none());
        assert_ne!(create.commit_id, save.commit_id);
        assert_ne!(save.commit_id, select.commit_id);
        assert_eq!(
            &create.frozen_commit_timestamp.as_str()[..10],
            create.frozen_evaluation_date.as_str()
        );
        let debug = format!("{create:?}");
        assert!(!debug.contains(create.commit_id.as_uuid().to_string().as_str()));
        assert!(!debug.contains(create.frozen_commit_timestamp.as_str()));
        assert!(!debug.contains(create.member_id.as_ref().unwrap().as_str()));
    }
}
