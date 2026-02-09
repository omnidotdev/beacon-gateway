//! Browser automation tool
//!
//! Provides CDP-based browser control for web automation tasks.
//! Uses `chromiumoxide` for Chrome `DevTools` Protocol integration.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::Page;
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::{Error, Result};

/// Browser automation controller
pub struct BrowserController {
    browser: Arc<Mutex<Option<Browser>>>,
    config: BrowserControllerConfig,
}

/// Configuration for browser controller
#[derive(Debug, Clone)]
pub struct BrowserControllerConfig {
    /// Path to Chrome/Chromium executable
    pub chrome_path: Option<PathBuf>,
    /// Run in headless mode
    pub headless: bool,
    /// User data directory for profiles
    pub user_data_dir: Option<PathBuf>,
    /// Default navigation timeout
    pub timeout: Duration,
    /// Window width
    pub width: u32,
    /// Window height
    pub height: u32,
}

impl Default for BrowserControllerConfig {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: true,
            user_data_dir: None,
            timeout: Duration::from_secs(30),
            width: 1280,
            height: 720,
        }
    }
}

/// Screenshot result
#[derive(Debug)]
pub struct Screenshot {
    /// PNG image data
    pub data: Vec<u8>,
    /// Format (always PNG)
    pub format: &'static str,
}

/// Page content result
#[derive(Debug)]
pub struct PageContent {
    /// Page URL
    pub url: String,
    /// Page title
    pub title: Option<String>,
    /// HTML content
    pub html: String,
    /// Extracted text content
    pub text: Option<String>,
}

/// Element info
#[derive(Debug, Clone)]
pub struct ElementInfo {
    /// Element tag name
    pub tag: String,
    /// Element text content
    pub text: Option<String>,
    /// Element attributes
    pub attributes: Vec<(String, String)>,
}

impl BrowserController {
    /// Create a new browser controller
    #[must_use]
    pub fn new(config: BrowserControllerConfig) -> Self {
        Self {
            browser: Arc::new(Mutex::new(None)),
            config,
        }
    }

    /// Launch the browser
    ///
    /// # Errors
    ///
    /// Returns error if browser fails to launch
    pub async fn launch(&self) -> Result<()> {
        let mut browser_config = BrowserConfig::builder();

        if self.config.headless {
            browser_config = browser_config.arg("--headless=new");
        }

        browser_config = browser_config
            .window_size(self.config.width, self.config.height)
            .arg("--disable-gpu")
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage");

        if let Some(ref chrome_path) = self.config.chrome_path {
            browser_config = browser_config.chrome_executable(chrome_path);
        }

        if let Some(ref user_data_dir) = self.config.user_data_dir {
            browser_config = browser_config.user_data_dir(user_data_dir);
        }

        let config = browser_config
            .build()
            .map_err(|e| Error::Browser(format!("Config error: {e}")))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| Error::Browser(format!("Launch failed: {e}")))?;

        // Spawn handler in background
        tokio::spawn(async move {
            while handler.next().await.is_some() {}
        });

        let mut guard = self.browser.lock().await;
        *guard = Some(browser);

        tracing::info!("Browser launched");
        Ok(())
    }

    /// Close the browser
    pub async fn close(&self) {
        let mut guard = self.browser.lock().await;
        if let Some(browser) = guard.take() {
            drop(browser);
            tracing::info!("Browser closed");
        }
    }

    /// Check if browser is running
    pub async fn is_running(&self) -> bool {
        self.browser.lock().await.is_some()
    }

    /// Navigate to a URL and return page content
    ///
    /// # Errors
    ///
    /// Returns error if navigation fails
    pub async fn navigate(&self, url: &str) -> Result<PageContent> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let page = browser
            .new_page(url)
            .await
            .map_err(|e| Error::Browser(format!("New page failed: {e}")))?;

        // Wait for navigation
        page.wait_for_navigation()
            .await
            .map_err(|e| Error::Browser(format!("Navigation failed: {e}")))?;

        self.get_page_content(&page).await
    }

    /// Take a screenshot of the current page
    ///
    /// # Errors
    ///
    /// Returns error if screenshot fails
    pub async fn screenshot(&self, url: Option<&str>) -> Result<Screenshot> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let page = if let Some(url) = url {
            let p = browser
                .new_page(url)
                .await
                .map_err(|e| Error::Browser(format!("New page failed: {e}")))?;
            p.wait_for_navigation()
                .await
                .map_err(|e| Error::Browser(format!("Navigation failed: {e}")))?;
            p
        } else {
            // Get current page
            let pages = browser
                .pages()
                .await
                .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;
            pages
                .into_iter()
                .next()
                .ok_or_else(|| Error::Browser("No active page".to_string()))?
        };

        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(true)
            .build();

        let data = page
            .screenshot(params)
            .await
            .map_err(|e| Error::Browser(format!("Screenshot failed: {e}")))?;

        Ok(Screenshot {
            data,
            format: "png",
        })
    }

    /// Execute JavaScript on the current page
    ///
    /// # Errors
    ///
    /// Returns error if execution fails
    pub async fn execute_js(&self, script: &str) -> Result<serde_json::Value> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let pages = browser
            .pages()
            .await
            .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;

        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| Error::Browser("No active page".to_string()))?;

        let result = page
            .evaluate(script)
            .await
            .map_err(|e| Error::Browser(format!("JS execution failed: {e}")))?;

        result
            .into_value()
            .map_err(|e| Error::Browser(format!("JS result parse failed: {e}")))
    }

    /// Click an element by selector
    ///
    /// # Errors
    ///
    /// Returns error if click fails
    pub async fn click(&self, selector: &str) -> Result<()> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let pages = browser
            .pages()
            .await
            .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;

        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| Error::Browser("No active page".to_string()))?;

        let element = page
            .find_element(selector)
            .await
            .map_err(|e| Error::Browser(format!("Element not found: {e}")))?;

        element
            .click()
            .await
            .map_err(|e| Error::Browser(format!("Click failed: {e}")))?;

        Ok(())
    }

    /// Type text into an element
    ///
    /// # Errors
    ///
    /// Returns error if typing fails
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<()> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let pages = browser
            .pages()
            .await
            .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;

        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| Error::Browser("No active page".to_string()))?;

        let element = page
            .find_element(selector)
            .await
            .map_err(|e| Error::Browser(format!("Element not found: {e}")))?;

        element
            .click()
            .await
            .map_err(|e| Error::Browser(format!("Focus failed: {e}")))?;

        element
            .type_str(text)
            .await
            .map_err(|e| Error::Browser(format!("Type failed: {e}")))?;

        Ok(())
    }

    /// Get elements matching a selector
    ///
    /// # Errors
    ///
    /// Returns error if query fails
    pub async fn query_selector_all(&self, selector: &str) -> Result<Vec<ElementInfo>> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let pages = browser
            .pages()
            .await
            .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;

        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| Error::Browser("No active page".to_string()))?;

        let elements = page
            .find_elements(selector)
            .await
            .map_err(|e| Error::Browser(format!("Query failed: {e}")))?;

        let mut results = Vec::new();
        for element in elements {
            // Get tag name via JS property
            let tag = element
                .string_property("tagName")
                .await
                .ok()
                .flatten()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();

            let text = element.inner_text().await.ok().flatten();

            // Get common attributes
            let mut attributes = Vec::new();
            for attr in ["id", "class", "href", "src", "name", "type", "value"] {
                if let Ok(Some(val)) = element.attribute(attr).await {
                    attributes.push((attr.to_string(), val));
                }
            }

            results.push(ElementInfo {
                tag,
                text,
                attributes,
            });
        }

        Ok(results)
    }

    /// Wait for an element to appear
    ///
    /// # Errors
    ///
    /// Returns error if element doesn't appear within timeout
    pub async fn wait_for_selector(&self, selector: &str) -> Result<()> {
        let guard = self.browser.lock().await;
        let browser = guard
            .as_ref()
            .ok_or_else(|| Error::Browser("Browser not running".to_string()))?;

        let pages = browser
            .pages()
            .await
            .map_err(|e| Error::Browser(format!("Get pages failed: {e}")))?;

        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| Error::Browser("No active page".to_string()))?;

        page.find_element(selector)
            .await
            .map_err(|e| Error::Browser(format!("Wait for selector failed: {e}")))?;

        Ok(())
    }

    /// Get page content helper
    async fn get_page_content(&self, page: &Page) -> Result<PageContent> {
        let url = page
            .url()
            .await
            .map_err(|e| Error::Browser(format!("Get URL failed: {e}")))?
            .unwrap_or_default();

        let title = page.get_title().await.ok().flatten();

        let html = page
            .content()
            .await
            .map_err(|e| Error::Browser(format!("Get content failed: {e}")))?;

        // Extract text content via JS
        let text = page
            .evaluate("document.body.innerText")
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok());

        Ok(PageContent {
            url,
            title,
            html,
            text,
        })
    }
}

impl Drop for BrowserController {
    fn drop(&mut self) {
        // Browser cleanup happens automatically when dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires Chrome to be installed"]
    async fn test_browser_launch() {
        let controller = BrowserController::new(BrowserControllerConfig::default());
        controller.launch().await.unwrap();
        assert!(controller.is_running().await);
        controller.close().await;
        assert!(!controller.is_running().await);
    }
}
