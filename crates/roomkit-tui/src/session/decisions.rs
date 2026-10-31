//! Pending human decisions, in delivery order. A new card on screen starts
//! unselected and disarmed.

use roomkit_tui_protocol::limits::MAX_PENDING_PERMISSIONS;
use roomkit_tui_protocol::{ConversationId, Permission, PermissionId};

use super::Session;
use super::bounds::permission_fits;

impl Session {
    pub(super) fn on_permission_requested(
        &mut self,
        conversation: &ConversationId,
        permission: Permission,
    ) {
        if !self.is_current(conversation) || self.active_run() != Some(&permission.run) {
            return;
        }
        if !permission_fits(&permission) || self.permissions().len() >= MAX_PENDING_PERMISSIONS {
            self.notice = "The host asked for an approval this view cannot display in full; \
                           it was not shown and will be refused when it expires"
                .into();
            return;
        }
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        if snapshot.permissions.iter().any(|p| p.id == permission.id) {
            return;
        }
        snapshot.permissions.push(permission);
        if snapshot.permissions.len() == 1 {
            self.show_next_permission();
        }
    }

    pub(super) fn on_permission_resolved(
        &mut self,
        conversation: &ConversationId,
        permission: &PermissionId,
    ) {
        if self.is_current(conversation) {
            self.retain_permissions(|p| p.id != *permission);
            self.deciding.remove(permission);
        }
    }

    /// Remove permissions; a new card on screen starts unselected and disarmed.
    pub(super) fn retain_permissions(&mut self, keep: impl Fn(&Permission) -> bool) {
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        let head = snapshot.permissions.first().map(|p| p.id.clone());
        snapshot.permissions.retain(keep);
        let changed = snapshot.permissions.first().map(|p| &p.id) != head.as_ref();
        let remaining: Vec<_> = snapshot.permissions.iter().map(|p| p.id.clone()).collect();
        self.deciding.retain(|id| remaining.contains(id));
        if changed {
            self.show_next_permission();
        }
    }

    fn show_next_permission(&mut self) {
        self.permission_scroll = 0;
        self.permission_choice = 0;
    }
}
