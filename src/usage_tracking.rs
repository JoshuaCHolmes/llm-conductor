use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::Result;
use chrono::{DateTime, Utc, Duration};
use serde::{Deserialize, Serialize};

use crate::types::ProviderId;

/// Different types of usage limits providers can have
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LimitType {
    /// No known limits (e.g., Outlier)
    Unlimited,
    /// Request-based limit (e.g., GitHub Copilot: 50/month, NVIDIA NIM: 40/min)
    RequestBased {
        max_requests: u64,
        current_requests: u64,
        reset_period: ResetPeriod,
        next_reset: DateTime<Utc>,
    },
    /// Token-based limit (e.g., TAMU: $5/day equivalent in tokens)
    TokenBased {
        max_tokens: u64,
        current_tokens: u64,
        reset_period: ResetPeriod,
        next_reset: DateTime<Utc>,
    },
    /// Cost-based limit (dollars or equivalent)
    CostBased {
        max_cost: f64,
        current_cost: f64,
        reset_period: ResetPeriod,
        next_reset: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ResetPeriod {
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Never,
}

impl ResetPeriod {
    pub fn next_reset_time(&self, from: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Minutely => from + Duration::minutes(1),
            Self::Hourly => from + Duration::hours(1),
            Self::Daily => from + Duration::days(1),
            Self::Weekly => from + Duration::weeks(1),
            Self::Monthly => {
                // Approximate - add 30 days
                from + Duration::days(30)
            }
            Self::Yearly => {
                // Approximate - add 365 days
                from + Duration::days(365)
            }
            Self::Never => DateTime::<Utc>::MAX_UTC,
        }
    }
    
    /// Get priority score (higher = more frequent resets = higher priority to use)
    pub fn priority_score(&self) -> u32 {
        match self {
            Self::Minutely => 1000,
            Self::Hourly => 500,
            Self::Daily => 100,
            Self::Weekly => 50,
            Self::Monthly => 10,
            Self::Yearly => 1,
            Self::Never => 0,
        }
    }
}

/// Provider-specific usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: ProviderId,
    pub limit_type: LimitType,
    pub last_updated: DateTime<Utc>,
    
    /// Historical usage for analytics
    #[serde(default)]
    pub history: Vec<UsageEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEntry {
    pub timestamp: DateTime<Utc>,
    pub requests: u64,
    pub tokens: u64,
    pub cost: f64,
}

impl ProviderUsage {
    pub fn new_unlimited(provider: ProviderId) -> Self {
        Self {
            provider,
            limit_type: LimitType::Unlimited,
            last_updated: Utc::now(),
            history: Vec::new(),
        }
    }
    
    pub fn new_request_based(
        provider: ProviderId,
        max_requests: u64,
        reset_period: ResetPeriod,
    ) -> Self {
        Self {
            provider,
            limit_type: LimitType::RequestBased {
                max_requests,
                current_requests: 0,
                reset_period,
                next_reset: reset_period.next_reset_time(Utc::now()),
            },
            last_updated: Utc::now(),
            history: Vec::new(),
        }
    }
    
    pub fn new_token_based(
        provider: ProviderId,
        max_tokens: u64,
        reset_period: ResetPeriod,
    ) -> Self {
        Self {
            provider,
            limit_type: LimitType::TokenBased {
                max_tokens,
                current_tokens: 0,
                reset_period,
                next_reset: reset_period.next_reset_time(Utc::now()),
            },
            last_updated: Utc::now(),
            history: Vec::new(),
        }
    }
    
    pub fn new_cost_based(
        provider: ProviderId,
        max_cost: f64,
        reset_period: ResetPeriod,
    ) -> Self {
        Self {
            provider,
            limit_type: LimitType::CostBased {
                max_cost,
                current_cost: 0.0,
                reset_period,
                next_reset: reset_period.next_reset_time(Utc::now()),
            },
            last_updated: Utc::now(),
            history: Vec::new(),
        }
    }
    
    /// Check if limit needs to be reset
    pub fn check_reset(&mut self) {
        let now = Utc::now();
        
        match &mut self.limit_type {
            LimitType::RequestBased { current_requests, next_reset, reset_period, .. } => {
                if now >= *next_reset {
                    *current_requests = 0;
                    *next_reset = reset_period.next_reset_time(now);
                }
            }
            LimitType::TokenBased { current_tokens, next_reset, reset_period, .. } => {
                if now >= *next_reset {
                    *current_tokens = 0;
                    *next_reset = reset_period.next_reset_time(now);
                }
            }
            LimitType::CostBased { current_cost, next_reset, reset_period, .. } => {
                if now >= *next_reset {
                    *current_cost = 0.0;
                    *next_reset = reset_period.next_reset_time(now);
                }
            }
            LimitType::Unlimited => {}
        }
        
        self.last_updated = now;
    }
    
    /// Record usage
    pub fn record_usage(&mut self, requests: u64, tokens: u64, cost: f64) {
        self.check_reset();
        
        match &mut self.limit_type {
            LimitType::RequestBased { current_requests, .. } => {
                *current_requests += requests;
            }
            LimitType::TokenBased { current_tokens, .. } => {
                *current_tokens += tokens;
            }
            LimitType::CostBased { current_cost, .. } => {
                *current_cost += cost;
            }
            LimitType::Unlimited => {}
        }
        
        // Add to history
        self.history.push(UsageEntry {
            timestamp: Utc::now(),
            requests,
            tokens,
            cost,
        });
        
        // Keep last 1000 entries only
        if self.history.len() > 1000 {
            self.history.drain(0..(self.history.len() - 1000));
        }
        
        self.last_updated = Utc::now();
    }
    
    /// Get remaining capacity as a percentage (0.0 to 1.0)
    pub fn remaining_capacity(&self) -> f64 {
        match &self.limit_type {
            LimitType::Unlimited => 1.0,
            LimitType::RequestBased { max_requests, current_requests, .. } => {
                if *max_requests == 0 {
                    0.0
                } else {
                    1.0 - (*current_requests as f64 / *max_requests as f64)
                }
            }
            LimitType::TokenBased { max_tokens, current_tokens, .. } => {
                if *max_tokens == 0 {
                    0.0
                } else {
                    1.0 - (*current_tokens as f64 / *max_tokens as f64)
                }
            }
            LimitType::CostBased { max_cost, current_cost, .. } => {
                if *max_cost == 0.0 {
                    0.0
                } else {
                    1.0 - (*current_cost / *max_cost)
                }
            }
        }
    }
    
    /// Check if provider is available (has capacity)
    pub fn is_available(&self) -> bool {
        self.remaining_capacity() > 0.0
    }
    
    /// Get priority score for provider selection
    /// Higher score = should be used preferentially
    pub fn priority_score(&self) -> f64 {
        let capacity = self.remaining_capacity();
        
        // If no capacity, score is 0
        if capacity <= 0.0 {
            return 0.0;
        }
        
        match &self.limit_type {
            // Unlimited gets highest score
            LimitType::Unlimited => 1000.0,
            
            // For limited providers, prioritize faster reset periods
            LimitType::RequestBased { reset_period, .. }
            | LimitType::TokenBased { reset_period, .. }
            | LimitType::CostBased { reset_period, .. } => {
                // Base score from reset frequency (faster reset = higher priority)
                let base_score = reset_period.priority_score() as f64;
                
                // Scale by remaining capacity (more capacity = higher priority)
                base_score * (0.5 + capacity * 0.5)
            }
        }
    }
}

/// Manages usage tracking for all providers
#[derive(Debug, Serialize, Deserialize)]
pub struct UsageTracker {
    providers: HashMap<ProviderId, ProviderUsage>,
    config_path: PathBuf,
}

impl UsageTracker {
    pub fn new(config_dir: &PathBuf) -> Result<Self> {
        let config_path = config_dir.join("usage.json");
        
        let providers = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        
        let mut tracker = Self {
            providers,
            config_path,
        };
        
        // Initialize default limits for known providers if not already set
        tracker.ensure_defaults();
        
        Ok(tracker)
    }
    
    /// Ensure default limits are set for all known providers
    fn ensure_defaults(&mut self) {
        // Only set if not already present
        if !self.providers.contains_key(&ProviderId::Outlier) {
            self.set_provider_limits(
                ProviderId::Outlier,
                ProviderUsage::new_unlimited(ProviderId::Outlier),
            );
        }
        
        if !self.providers.contains_key(&ProviderId::GitHubCopilot) {
            self.set_provider_limits(
                ProviderId::GitHubCopilot,
                ProviderUsage::new_request_based(ProviderId::GitHubCopilot, 50, ResetPeriod::Monthly),
            );
        }
        
        if !self.providers.contains_key(&ProviderId::Tamu) {
            self.set_provider_limits(
                ProviderId::Tamu,
                ProviderUsage::new_token_based(ProviderId::Tamu, 500_000, ResetPeriod::Daily),
            );
        }
        
        if !self.providers.contains_key(&ProviderId::NvidiaNim) {
            self.set_provider_limits(
                ProviderId::NvidiaNim,
                ProviderUsage::new_request_based(ProviderId::NvidiaNim, 40, ResetPeriod::Minutely),
            );
        }
        
        if !self.providers.contains_key(&ProviderId::Ollama) {
            self.set_provider_limits(
                ProviderId::Ollama,
                ProviderUsage::new_unlimited(ProviderId::Ollama),
            );
        }
    }
    
    /// Initialize or update provider limits
    pub fn set_provider_limits(&mut self, provider: ProviderId, usage: ProviderUsage) {
        self.providers.insert(provider, usage);
        let _ = self.save();
    }
    
    /// Record usage for a provider
    pub fn record_usage(&mut self, provider: ProviderId, requests: u64, tokens: u64, cost: f64) {
        if let Some(usage) = self.providers.get_mut(&provider) {
            usage.record_usage(requests, tokens, cost);
            let _ = self.save();
        }
    }
    
    /// Get usage info for a provider
    pub fn get_usage(&mut self, provider: &ProviderId) -> Option<&mut ProviderUsage> {
        if let Some(usage) = self.providers.get_mut(provider) {
            usage.check_reset();
            Some(usage)
        } else {
            None
        }
    }
    
    /// Get all provider usages sorted by priority
    pub fn get_prioritized_providers(&mut self) -> Vec<(ProviderId, f64)> {
        // Check resets for all providers
        for usage in self.providers.values_mut() {
            usage.check_reset();
        }
        
        let mut providers: Vec<_> = self.providers
            .iter()
            .map(|(id, usage)| (id.clone(), usage.priority_score()))
            .collect();
        
        // Sort by priority (highest first)
        providers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        providers
    }
    
    /// Save usage data to disk
    fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.providers)?;
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self {
            providers: HashMap::new(),
            config_path: PathBuf::from("."),
        }
    }
}
