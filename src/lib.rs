//! Measure a computer's power draw and price it with an electricity tariff.

pub mod cli;
pub mod collector;
pub mod config;
pub mod energy;
pub mod extension;
pub mod pricing;
pub mod providers;
pub mod report;
pub mod reprice;
pub mod sample;
pub mod series;
pub mod setup;
pub mod store;
pub mod tariff;
pub mod tariff_form;
pub mod time_range;

#[cfg(test)]
mod test_support;
