use super::*;

impl AdminApp {
    fn push_client_online_toast(&mut self, title: String, detail: String) {
        self.push_log(format!("client online: {title}"));
        self.client_online_toasts.push_back(ClientOnlineToast {
            title,
            detail,
            created_at: Instant::now(),
        });
        while self.client_online_toasts.len() > MAX_CLIENT_ONLINE_TOASTS {
            self.client_online_toasts.pop_front();
        }
    }

    pub(super) fn merge_clients(&mut self, clients: Vec<ClientInfo>) {
        let notify_online_changes = self.client_list_initialized;
        let online_ids: HashSet<String> = clients.iter().map(|client| client.id.clone()).collect();
        let mut online_notices = Vec::new();
        for client in clients {
            if let Some(existing) = self.clients.iter_mut().find(|row| row.info.id == client.id) {
                let was_online = existing.status == ClientStatus::Online;
                existing.info = client;
                existing.status = ClientStatus::Online;
                if !was_online {
                    online_notices.push(client_online_notice(&existing.info));
                }
            } else {
                online_notices.push(client_online_notice(&client));
                self.clients.push(ClientRow {
                    info: client,
                    status: ClientStatus::Online,
                });
            }
        }

        for row in &mut self.clients {
            if !online_ids.contains(&row.info.id) && row.status != ClientStatus::Stale {
                row.status = ClientStatus::Offline;
            }
        }

        if notify_online_changes {
            for (title, detail) in online_notices {
                self.push_client_online_toast(title, detail);
            }
        }
        self.client_list_initialized = true;
    }

    pub(super) fn filtered_clients(&self) -> Vec<ClientRow> {
        let filter = self.client_filter.trim().to_ascii_lowercase();
        self.clients
            .iter()
            .filter(|row| {
                if filter.is_empty() {
                    return true;
                }
                row.info.id.to_ascii_lowercase().contains(&filter)
                    || row.info.fingerprint.to_ascii_lowercase().contains(&filter)
                    || row.info.hostname.to_ascii_lowercase().contains(&filter)
                    || row.info.username.to_ascii_lowercase().contains(&filter)
                    || row.info.os.to_ascii_lowercase().contains(&filter)
                    || self
                        .client_display_label(row)
                        .to_ascii_lowercase()
                        .contains(&filter)
                    || self
                        .client_group(&row.info.id)
                        .to_ascii_lowercase()
                        .contains(&filter)
                    || client_location_label(&row.info)
                        .to_ascii_lowercase()
                        .contains(&filter)
            })
            .cloned()
            .collect()
    }

    pub(super) fn online_client_count(&self) -> usize {
        self.clients
            .iter()
            .filter(|row| row.status == ClientStatus::Online)
            .count()
    }

    pub(super) fn client_status_for(&self, client_id: &str) -> Option<ClientStatus> {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| row.status)
    }

    pub(super) fn selected_client_label(&self) -> String {
        let Some(selected_id) = self.selected_client_id.as_deref() else {
            return "None".to_string();
        };

        self.clients
            .iter()
            .find(|row| row.info.id == selected_id)
            .map(|row| self.client_display_label(row))
            .unwrap_or_else(|| selected_id.to_string())
    }

    pub(super) fn client_display_label(&self, row: &ClientRow) -> String {
        self.client_aliases
            .get(&row.info.id)
            .cloned()
            .unwrap_or_else(|| client_identity_label(&row.info))
    }
}
