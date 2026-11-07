//! Clearing and listing the website data of one persistent profile.
//!
//! The call is admitted for the exact profile it names, only while that
//! profile is durable and not being erased, and reaches only that profile's
//! own engine store. A private partition, another profile and Work's own
//! store are refused or simply never addressed.

use std::collections::{HashMap, HashSet};

use zephium_core::browsing_data::{
    EngineDataCall, EngineDataCompletion, EngineDataFailure, EngineDataReply,
};
use zephium_core::ids::ProfileId;
use zephium_core::ports::engine::Partition;

use super::profiles::{bind_profile_persistence_class, ProfilePersistenceClass};
use super::EngineHost;

/// The profile a data call may act on, or why none.
pub(super) fn admitted_profile(
    partition: Partition,
    classes: &mut HashMap<ProfileId, ProfilePersistenceClass>,
    erasing: &HashSet<ProfileId>,
) -> Result<ProfileId, EngineDataFailure> {
    let profile = match partition {
        Partition::Default(profile) | Partition::Persistent(profile) => profile,
        Partition::Ephemeral(_) => return Err(EngineDataFailure::Unsupported),
    };
    if erasing.contains(&profile) {
        return Err(EngineDataFailure::Unavailable);
    }
    // A profile already bound private never gains a durable store here.
    if !bind_profile_persistence_class(classes, partition) {
        return Err(EngineDataFailure::Unsupported);
    }
    Ok(profile)
}

impl EngineHost {
    pub(crate) fn browsing_data_call(
        &mut self,
        partition: Partition,
        call: EngineDataCall,
        done: EngineDataCompletion,
    ) {
        let profile = match admitted_profile(
            partition,
            &mut self.profile_persistence_classes,
            &self.erasure_tombstones,
        ) {
            Ok(profile) => profile,
            Err(failure) => {
                done.finish(EngineDataReply::Failed(failure));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        {
            use crate::platform::imp::browsing_data as native;
            match call {
                EngineDataCall::Clear { kinds, since } => {
                    native::clear(profile, kinds, since, done)
                }
                EngineDataCall::Sites => native::sites(profile, done),
                EngineDataCall::ForgetSite { site, .. } => native::forget_site(profile, site, done),
            }
        }
        #[cfg(target_os = "windows")]
        self.windows_browsing_data(partition, profile, call, done);
    }
}

#[cfg(target_os = "windows")]
impl EngineHost {
    fn windows_browsing_data(
        &mut self,
        partition: Partition,
        profile: ProfileId,
        call: EngineDataCall,
        done: EngineDataCompletion,
    ) {
        use crate::platform::imp::browsing_data as native;
        let Some((target, temporary)) = self.windows_data_target(partition) else {
            done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
            return;
        };
        // A page opened only to reach the profile goes again once it answers.
        let done = if temporary {
            EngineDataCompletion::new(move |reply| {
                done.finish(reply);
                let _ = super::try_with(move |host| host.close_idle_spare(profile));
            })
        } else {
            done
        };
        match call {
            EngineDataCall::Clear { kinds, since } => native::clear(&target, kinds, since, done),
            EngineDataCall::Sites => native::sites(&target, done),
            EngineDataCall::ForgetSite { site, origin } => {
                native::forget_site(&target, site, origin, done)
            }
        }
    }

    /// A live view of the profile, which is how WebView2 reaches its data. A
    /// profile with no page open gets a hidden one for the call, and the flag
    /// says so.
    fn windows_data_target(
        &mut self,
        partition: Partition,
    ) -> Option<(crate::platform::imp::browsing_data::DataTarget, bool)> {
        use crate::platform::imp::browsing_data::DataTarget;
        let profile = partition.profile();
        let open = self
            .views
            .iter()
            .find(|(id, _)| {
                self.partitions
                    .get(id)
                    .is_some_and(|owner| owner.profile() == profile)
            })
            .map(|(_, view)| view)
            .or_else(|| {
                self.spare
                    .as_ref()
                    .filter(|spare| spare.partition.profile() == profile)
                    .map(|spare| &spare.view)
            });
        if let Some(view) = open {
            return DataTarget::of(view).ok().map(|target| (target, false));
        }
        self.applied_content_policy(profile)?;
        // One warm page exists at a time; another profile's is only a
        // latency optimisation.
        self.spare = None;
        let id = std::rc::Rc::new(std::cell::Cell::new(zephium_core::ids::ItemId::generate()));
        let view = self.build_view(
            id.clone(),
            partition,
            "about:blank",
            zephium_core::geometry::Rect::default(),
            super::permits::EventPermit::inactive(),
            super::construction::NativeViewPurpose::WarmSpare,
        )?;
        let target = DataTarget::of(&view).ok();
        self.spare = Some(super::Spare {
            partition,
            view,
            id,
        });
        if target.is_none() {
            self.close_idle_spare(profile);
        }
        target.map(|target| (target, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(
        profile: ProfileId,
        class: ProfilePersistenceClass,
    ) -> HashMap<ProfileId, ProfilePersistenceClass> {
        HashMap::from([(profile, class)])
    }

    #[test]
    fn a_durable_profile_is_admitted_as_itself_and_no_other() {
        let profile = ProfileId::generate();
        let other = ProfileId::generate();
        let mut classes = bound(other, ProfilePersistenceClass::Durable);
        let erasing = HashSet::new();
        for partition in [Partition::Persistent(profile), Partition::Default(profile)] {
            assert_eq!(
                admitted_profile(partition, &mut classes, &erasing),
                Ok(profile)
            );
        }
        assert_eq!(
            classes.get(&profile),
            Some(&ProfilePersistenceClass::Durable)
        );
        assert_eq!(classes.get(&other), Some(&ProfilePersistenceClass::Durable));
    }

    #[test]
    fn a_private_partition_is_refused_and_binds_nothing() {
        let profile = ProfileId::generate();
        let mut classes = HashMap::new();
        assert_eq!(
            admitted_profile(Partition::Ephemeral(profile), &mut classes, &HashSet::new()),
            Err(EngineDataFailure::Unsupported)
        );
        assert!(classes.is_empty());
    }

    #[test]
    fn a_profile_bound_private_never_gets_a_durable_store() {
        let profile = ProfileId::generate();
        let mut classes = bound(profile, ProfilePersistenceClass::Ephemeral);
        assert_eq!(
            admitted_profile(
                Partition::Persistent(profile),
                &mut classes,
                &HashSet::new()
            ),
            Err(EngineDataFailure::Unsupported)
        );
        assert_eq!(
            classes.get(&profile),
            Some(&ProfilePersistenceClass::Ephemeral)
        );
    }

    #[test]
    fn an_erasing_profile_is_refused() {
        let profile = ProfileId::generate();
        let mut classes = HashMap::new();
        let erasing = HashSet::from([profile]);
        assert_eq!(
            admitted_profile(Partition::Persistent(profile), &mut classes, &erasing),
            Err(EngineDataFailure::Unavailable)
        );
        assert!(classes.is_empty());
    }
}
