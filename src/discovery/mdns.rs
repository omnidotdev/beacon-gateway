//! mDNS service advertisement
//!
//! Advertises the beacon gateway using mDNS (multicast DNS) so that
//! local clients can discover the gateway automatically
//!
//! Service type: `_beacon-gateway._tcp.local`
//! Instance name: `{persona}-{device_id_short}`
//!
//! TXT records:
//! - `version`: Gateway version
//! - `device_id`: Full device ID
//! - `persona`: Active persona ID
//! - `voice`: Whether voice is supported ("true"/"false")
//! - `tls`: Whether TLS is enabled ("true"/"false")

use std::collections::HashMap;
use std::sync::Arc;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tokio::sync::RwLock;

use crate::Result;

/// mDNS service type for beacon gateway
pub const SERVICE_TYPE: &str = "_beacon-gateway._tcp.local.";

/// mDNS advertiser for beacon gateway discovery
pub struct MdnsAdvertiser {
    /// mDNS daemon
    daemon: ServiceDaemon,

    /// Currently registered service (if any)
    registered_service: Arc<RwLock<Option<String>>>,
}

impl MdnsAdvertiser {
    /// Create a new mDNS advertiser
    ///
    /// # Errors
    ///
    /// Returns error if mDNS daemon cannot be created
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| crate::Error::Config(format!("failed to create mDNS daemon: {e}")))?;

        Ok(Self {
            daemon,
            registered_service: Arc::new(RwLock::new(None)),
        })
    }

    /// Start advertising the gateway
    ///
    /// # Arguments
    ///
    /// * `persona_id` - The active persona ID
    /// * `device_id` - The gateway's device ID
    /// * `port` - The HTTP API port
    /// * `voice_enabled` - Whether voice is supported
    /// * `tls_enabled` - Whether TLS is enabled
    ///
    /// # Errors
    ///
    /// Returns error if service cannot be registered
    pub async fn start(
        &self,
        persona_id: &str,
        device_id: &str,
        port: u16,
        voice_enabled: bool,
        tls_enabled: bool,
    ) -> Result<()> {
        // Build instance name: {persona}-{device_id_short}
        let device_id_short = &device_id[..8.min(device_id.len())];
        let instance_name = format!("{persona_id}-{device_id_short}");

        // Get hostname
        let hostname = hostname::get()
            .map_or_else(|_| "beacon".to_string(), |h| h.to_string_lossy().to_string());

        // Build TXT record properties
        let mut properties = HashMap::new();
        properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        properties.insert("device_id".to_string(), device_id.to_string());
        properties.insert("persona".to_string(), persona_id.to_string());
        properties.insert("voice".to_string(), voice_enabled.to_string());
        properties.insert("tls".to_string(), tls_enabled.to_string());

        // Create service info
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{hostname}.local."),
            "",
            port,
            properties,
        )
        .map_err(|e| crate::Error::Config(format!("failed to create service info: {e}")))?;

        // Get the full name before registering
        let fullname = service.get_fullname().to_string();

        // Register the service
        self.daemon
            .register(service)
            .map_err(|e| crate::Error::Config(format!("failed to register mDNS service: {e}")))?;

        // Store the registered service name
        {
            let mut registered = self.registered_service.write().await;
            *registered = Some(fullname.clone());
        }

        tracing::info!(
            service_type = SERVICE_TYPE,
            instance = instance_name,
            port = port,
            "mDNS service registered"
        );

        Ok(())
    }

    /// Stop advertising the gateway
    pub async fn stop(&self) {
        let fullname = {
            let mut registered = self.registered_service.write().await;
            registered.take()
        };

        if let Some(name) = fullname {
            if let Err(e) = self.daemon.unregister(&name) {
                tracing::warn!(error = %e, "failed to unregister mDNS service");
            } else {
                tracing::info!("mDNS service unregistered");
            }
        }
    }

    /// Check if currently advertising
    pub async fn is_advertising(&self) -> bool {
        self.registered_service.read().await.is_some()
    }
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        // Try to unregister on drop (best effort, synchronous)
        if let Ok(guard) = self.registered_service.try_read() {
            if let Some(name) = guard.as_ref() {
                let _ = self.daemon.unregister(name);
            }
        }
        // Shutdown the daemon
        if let Err(e) = self.daemon.shutdown() {
            tracing::trace!(error = %e, "mDNS daemon shutdown error (expected on normal exit)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_type_format() {
        // Service type should end with ".local."
        assert!(SERVICE_TYPE.ends_with(".local."));
        // Should start with underscore
        assert!(SERVICE_TYPE.starts_with('_'));
        // Should be TCP
        assert!(SERVICE_TYPE.contains("._tcp."));
    }

    #[tokio::test]
    async fn test_advertiser_creation() {
        // Just test that we can create an advertiser
        // Actual mDNS registration may fail in CI environments
        let result = MdnsAdvertiser::new();

        // On systems with mDNS support, this should succeed
        // On others (like CI), it might fail - that's OK
        if let Ok(advertiser) = result {
            assert!(!advertiser.is_advertising().await);
        }
    }
}
