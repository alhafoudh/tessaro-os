//! Who may manage the device: the claim, tokens, the root password, the SSH
//! keys that log in as root, and the factory reset that clears them all.
//! See `auth.rs` for the claim model itself.

use std::collections::BTreeMap;

use protocol::sshkey::PublicKey;
use protocol::{
    Claimed, Done, Password, SshAccess, SshKeyInfo, SshKeyRevoked, TokenCreated, TokenInfo,
};

use super::{After, Caller, Control, Reply, UNCLAIMED};
use crate::auth;
use crate::deadline::blocking;
use crate::files;
use crate::shadow;
use crate::ssh;
use crate::sync::lock;

impl Control {
    pub(super) async fn claim(&self, caller: &Caller, name: &str) -> Result<Claimed, String> {
        if matches!(caller, Caller::Token { .. }) {
            return Err("this device is already claimed".to_string());
        }
        let name = auth::check_name(name)?;

        let _writes = self.writes.lock().await;
        if self.claimed() {
            return Err("this device is already claimed".to_string());
        }

        // The password first, then the token: a power cut in between leaves
        // no token and a password, which the boot oneshot puts back to empty.
        // The other order would leave a claimed device with an empty root.
        let password = auth::random_password()?;
        self.set_root(Some(password.clone())).await?;

        let issued = self
            .update_auth("updating auth.json", move |auth| {
                if auth.claimed() {
                    return Err("this device is already claimed".to_string());
                }
                auth.issue(&name, "claim")
            })
            .await;
        let (entry, secret) = match issued {
            Ok(issued) => issued,
            Err(err) => {
                // Put the invariant back: unclaimed means an empty password.
                let _ = self.set_root(None).await;
                return Err(err);
            }
        };

        self.announce_claimed(true);
        self.log.info(format!(
            "claimed by {} as {:?} (token {}); root password set",
            caller.describe(),
            entry.name,
            entry.id
        ));

        // The hotspot's password last, after the claim that decides it: a
        // power cut before it leaves a claimed device with an open hotspot
        // until `net wifi hotspot-password`, never an unclaimed one with a
        // password nobody was shown. Applied once the answer is out.
        let hotspot = match self.set_hotspot_psk().await {
            Ok(hotspot) => hotspot,
            Err(err) => {
                self.log
                    .info(format!("claim: the hotspot keeps no password: {err}"));
                None
            }
        };

        Ok(Claimed {
            token_id: entry.id,
            token: secret,
            root_password: password,
            hotspot,
        })
    }

    pub(super) async fn token_create(
        &self,
        caller: &Caller,
        name: &str,
    ) -> Result<TokenCreated, String> {
        let name = auth::check_name(name)?;
        let issuer = match caller {
            Caller::Local => "local".to_string(),
            Caller::Token { id, .. } => id.clone(),
            Caller::Anonymous { .. } | Caller::Page => {
                return Err("a token is needed to issue a token".to_string())
            }
        };

        let _writes = self.writes.lock().await;
        self.require_claimed("")?;

        let (entry, secret) = self
            .update_auth("updating auth.json", move |auth| auth.issue(&name, &issuer))
            .await?;

        self.log.info(format!(
            "token {} ({:?}) issued by {}",
            entry.id,
            entry.name,
            caller.describe()
        ));
        Ok(TokenCreated {
            id: entry.id,
            token: secret,
        })
    }

    pub(super) fn token_list(&self) -> Vec<TokenInfo> {
        lock(&self.auth)
            .tokens
            .iter()
            .map(|entry| TokenInfo {
                id: entry.id.clone(),
                name: entry.name.clone(),
                issued_by: entry.issued_by.clone(),
            })
            .collect()
    }

    pub(super) async fn token_revoke(&self, caller: &Caller, id: &str) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let wanted = id.to_string();
        let (entry, now_unclaimed) = self
            .update_auth("updating auth.json", move |auth| {
                let entry = auth
                    .revoke(&wanted)
                    .ok_or_else(|| format!("no token {wanted}"))?;
                Ok((entry, !auth.claimed()))
            })
            .await?;
        self.log.info(format!(
            "token {} ({:?}) revoked by {}",
            entry.id,
            entry.name,
            caller.describe()
        ));

        if now_unclaimed {
            self.announce_claimed(false);
            self.clear_credentials().await?;
            self.log.info(
                "the last token was revoked: unclaimed, ssh keys removed, root password emptied, hotspot open",
            );
            return Ok(Done::new(format!(
                "revoked {}; that was the last token, the device is unclaimed",
                entry.id
            )));
        }
        Ok(Done::new(format!("revoked {}", entry.id)))
    }

    pub(super) async fn password_set(
        &self,
        caller: &Caller,
        password: Option<String>,
    ) -> Result<Password, String> {
        let _writes = self.writes.lock().await;
        self.require_claimed("an unclaimed device keeps an empty root password")?;

        let (password, generated) = match password {
            Some(password) => {
                protocol::check_password(&password)?;
                (password, false)
            }
            None => (auth::random_password()?, true),
        };
        self.set_root(Some(password.clone())).await?;
        self.log
            .info(format!("root password changed by {}", caller.describe()));

        Ok(Password {
            password: generated.then_some(password),
        })
    }

    pub(super) async fn unclaim(&self, caller: &Caller) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        self.drop_claim().await?;
        self.log.info(format!(
            "unclaimed by {}: every token and ssh key removed, root password emptied",
            caller.describe()
        ));
        Ok(Done::new(
            "unclaimed: every token and ssh key is gone and the root password is empty",
        ))
    }

    /// Refused on an unclaimed device, with `why` it matters, if given.
    pub(super) fn require_claimed(&self, why: &str) -> Result<(), String> {
        if self.claimed() {
            Ok(())
        } else if why.is_empty() {
            Err(UNCLAIMED.to_string())
        } else {
            Err(format!(
                "this device is unclaimed, and {why}; `tessaro-ctl access claim` it first"
            ))
        }
    }

    /// Tokens first, then the other credentials - the order that a power cut
    /// in between leaves healable: the boot oneshot empties them on a device
    /// with no tokens (see `claim`).
    async fn drop_claim(&self) -> Result<(), String> {
        self.update_auth("updating auth.json", |auth| {
            auth.tokens.clear();
            Ok(())
        })
        .await?;
        self.announce_claimed(false);
        self.clear_credentials().await
    }

    /// What goes with the last token: SSH keys, then the root password, then
    /// the hotspot's password - unclaimed means an open hotspot, like an
    /// empty root password. The profiles follow once the answer is out
    /// (`After::Network`).
    async fn clear_credentials(&self) -> Result<(), String> {
        let path = self.paths.authorized_keys.clone();
        blocking("emptying authorized_keys", move || {
            ssh::clear(&path).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        self.set_root(None).await?;
        self.update_secrets(|secrets| secrets.hotspot_psk = None)
            .await
    }

    pub(super) async fn ssh_authorize(
        &self,
        caller: &Caller,
        key: &str,
    ) -> Result<SshAccess, String> {
        let _writes = self.writes.lock().await;
        self.require_claimed("an unclaimed device has no credentials")?;
        let key = PublicKey::parse(key)?;
        let fingerprint = key.fingerprint();

        let path = self.paths.authorized_keys.clone();
        let dirs = self.paths.ssh_host_key_dirs.clone();
        let (added, generated, host_keys) = blocking("updating authorized_keys", move || {
            let added =
                ssh::add(&path, &key).map_err(|err| format!("{}: {err}", path.display()))?;
            let generated = ssh::ensure_host_key(&dirs);
            Ok((added, generated, ssh::host_keys(&dirs)))
        })
        .await?;
        if let Err(err) = generated {
            self.log
                .info(format!("no ssh host key, and making one failed: {err}"));
        }

        if added {
            self.log.info(format!(
                "ssh key {fingerprint} authorized by {}",
                caller.describe()
            ));
        }
        if host_keys.is_empty() {
            self.log
                .info("no ssh host key could be read; the client will ask about it");
        }
        Ok(SshAccess {
            fingerprint,
            added,
            host_keys,
        })
    }

    pub(super) async fn ssh_key_list(&self) -> Result<Vec<SshKeyInfo>, String> {
        let path = self.paths.authorized_keys.clone();
        let keys = blocking("reading authorized_keys", move || {
            ssh::list(&path).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        Ok(keys
            .into_iter()
            .map(|key| SshKeyInfo {
                fingerprint: key.fingerprint(),
                kind: key.kind,
                comment: key.comment,
            })
            .collect())
    }

    pub(super) async fn ssh_key_revoke(
        &self,
        caller: &Caller,
        query: &str,
    ) -> Result<SshKeyRevoked, String> {
        let _writes = self.writes.lock().await;
        let path = self.paths.authorized_keys.clone();
        let query = query.to_string();
        let key = blocking("updating authorized_keys", move || {
            ssh::remove(&path, &query)
        })
        .await?;
        let fingerprint = key.fingerprint();
        self.log.info(format!(
            "ssh key {fingerprint} ({:?}) revoked by {}",
            key.comment,
            caller.describe()
        ));
        Ok(SshKeyRevoked {
            message: format!("revoked {fingerprint}"),
            fingerprint: Some(fingerprint),
        })
    }

    pub(super) async fn factory_reset(&self, caller: &Caller) -> Reply {
        // The reset re-renders the profiles, and a change mid-way would roll
        // back onto them.
        if self.network.busy() {
            return Reply::err("a network change is in progress; reset once it is done");
        }
        let _writes = self.writes.lock().await;
        if let Err(err) = self.drop_claim().await {
            return Reply::err(err);
        }

        let store = self.state.clone();
        if let Err(err) = blocking("removing state.json", move || {
            store.remove().map_err(|err| err.to_string())
        })
        .await
        {
            return Reply::err(err);
        }
        *lock(&self.probation) = None;
        let secrets = self.secrets.clone();
        if let Err(err) = blocking("removing secrets.json", move || {
            secrets.remove().map_err(|err| err.to_string())
        })
        .await
        {
            return Reply::err(err);
        }
        let paths = self.paths.clone();
        if let Err(err) = blocking("emptying the file store", move || {
            files::wipe(&paths).map_err(|err| err.to_string())
        })
        .await
        {
            // The store is renamed away first, so what is left is only the
            // deleting, which the next boot finishes.
            self.log
                .info(format!("factory reset: the file store: {err}"));
        }

        if let Err(err) = self.render(&BTreeMap::new()).await {
            return Reply::err(format!("reset, but rendering failed: {err}"));
        }
        // The profiles now say the defaults: DHCP, an open hotspot. What is
        // up keeps running until the next boot brings them up afresh.
        self.refresh_network().await;

        self.log.info(format!(
            "factory reset by {}: settings, tokens, ssh keys, passwords, the network and stored files cleared",
            caller.describe()
        ));
        // Weston takes the browser and the agent with it (PartOf=), so every
        // consumer comes back up on the defaults.
        Reply::ok(Done::new(
            "factory reset: defaults restored, unclaimed, stored files removed, \
             restarting the display; \
             the network is DHCP and an open hotspot from the next boot",
        ))
        .then(Some(After::Restart(self.paths.weston_unit.clone())))
    }

    async fn set_root(&self, password: Option<String>) -> Result<(), String> {
        let path = self.paths.shadow.clone();
        blocking("updating /etc/shadow", move || {
            let hashed = match password {
                Some(password) => Some(shadow::hash(&password).map_err(|err| err.to_string())?),
                None => None,
            };
            shadow::set_root(&path, hashed.as_deref())
                .map_err(|err| format!("{}: {err}", path.display()))
        })
        .await
    }

    fn announce_claimed(&self, claimed: bool) {
        if let Some(mdns) = lock(&self.mdns).as_ref() {
            mdns.set_claimed(claimed, &self.log);
        }
        self.welcome.notify_one();
    }
}
