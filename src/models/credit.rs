use serde::{Deserialize, Serialize};
use rand::Rng;
use chrono::{NaiveDate, Utc};
use once_cell::sync::Lazy;

// Name datasets loaded from files
static MALE_FIRST_NAMES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    parse_names(include_str!("../data_preparation/Nama Depan Laki-Laki.txt"))
});

static MALE_LAST_NAMES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    parse_names(include_str!("../data_preparation/Nama Belakang Laki-Laki.txt"))
});

static FEMALE_FIRST_NAMES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    parse_names(include_str!("../data_preparation/Nama Depan Perempuan.txt"))
});

static FEMALE_LAST_NAMES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    parse_names(include_str!("../data_preparation/Nama Belakang Perempuan.txt"))
});

fn parse_names(content: &str) -> Vec<&str> {
    content
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Credit record representing SLIK credit data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditRecord {
    /// Unique record identifier
    pub id: String,
    /// NIK (Nomor Induk Kependudukan) - Indonesian ID number
    pub nik: String,
    /// Full name
    pub nama_lengkap: String,
    /// Credit facility type
    pub jenis_fasilitas: String,
    /// Credit amount (minor units: rupiah)
    pub jumlah_kredit: i64,
    /// Currency code
    pub mata_uang: String,
    /// Interest rate (bps: basis points). Example: 750 = 7.50%
    pub suku_bunga_bps: i32,
    /// Start date
    pub tanggal_mulai: String,
    /// End date
    pub tanggal_jatuh_tempo: String,
    /// Outstanding balance (minor units: rupiah)
    pub saldo_outstanding: i64,
    /// Collectability status (1-5)
    pub kolektabilitas: u8,
    /// Branch code
    pub kode_cabang: String,
    /// Account officer
    pub account_officer: String,
    /// Last update timestamp (RFC3339)
    pub last_updated: String,
    /// Whether this record has intentional errors (for testing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _has_error: Option<bool>,
    /// Type of error injected (for testing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _error_type: Option<String>,
}

impl CreditRecord {
    /// Generate a clean (valid) credit record
    pub fn generate_clean(rng: &mut impl Rng, id: u64) -> Self {
        let nik = format!("{:016}", rng.gen_range(1_000_000_000_000_000u64..9_999_999_999_999_999u64));

        let is_male = rng.gen::<bool>();
        let (first_names, last_names) = if is_male {
            (&MALE_FIRST_NAMES, &MALE_LAST_NAMES)
        } else {
            (&FEMALE_FIRST_NAMES, &FEMALE_LAST_NAMES)
        };

        let first = first_names[rng.gen_range(0..first_names.len())];
        let last = last_names[rng.gen_range(0..last_names.len())];
        let nama_lengkap = format!("{} {}", first, last);

        let facilities = ["KPR", "KKB", "KMK", "KI", "KUR", "Kartu Kredit"];
        let branches = ["JKT001", "JKT002", "BDG001", "SBY001", "MDN001", "MKS001"];

        let start_date = NaiveDate::from_ymd_opt(
            2020 + rng.gen_range(0..4),
            rng.gen_range(1..13),
            rng.gen_range(1..28),
        )
        .unwrap();

        let end_date = start_date + chrono::Duration::days(rng.gen_range(365..3650));

        // Use integer rupiah for safety.
        let kredit = rng.gen_range(10_000_000i64..10_000_000_000i64);
        let outstanding = rng.gen_range((kredit / 10).max(1)..=kredit); // <= kredit

        // deterministic-ish last_updated (but still looks time-like)
        let last_day = (id % 28) + 1;
        let last_updated = format!("2024-01-{:02}T10:00:00Z", last_day);

        Self {
            id: format!("CR{:010}", id),
            nik,
            nama_lengkap,
            jenis_fasilitas: facilities[rng.gen_range(0..facilities.len())].to_string(),
            jumlah_kredit: kredit,
            mata_uang: "IDR".to_string(),
            suku_bunga_bps: rng.gen_range(500..1500), // 5.00%..15.00%
            tanggal_mulai: start_date.format("%Y-%m-%d").to_string(),
            tanggal_jatuh_tempo: end_date.format("%Y-%m-%d").to_string(),
            saldo_outstanding: outstanding,
            kolektabilitas: rng.gen_range(1..6),
            kode_cabang: branches[rng.gen_range(0..branches.len())].to_string(),
            account_officer: format!("AO{:04}", rng.gen_range(1..1000)),
            last_updated,
            _has_error: None,
            _error_type: None,
        }
    }

    /// Inject an error into the record
    pub fn inject_error(mut self, rng: &mut impl Rng, error_type: &str) -> Self {
        self._has_error = Some(true);
        self._error_type = Some(error_type.to_string());

        match error_type {
            "invalid_nik" => {
                let choice = rng.gen_range(0..3);
                self.nik = match choice {
                    0 => format!("{:010}", rng.gen_range(1_000_000_000u64..9_999_999_999u64)),
                    1 => format!("ABC{:013}", rng.gen_range(1_000_000_000_000u64..9_999_999_999_999u64)),
                    _ => "".to_string(),
                };
            }
            "negative_amount" => {
                self.jumlah_kredit = -rng.gen_range(1_000i64..100_000_000i64);
            }
            "invalid_date" => {
                let choice = rng.gen_range(0..3);
                match choice {
                    0 => self.tanggal_mulai = "2024-13-45".to_string(),
                    1 => self.tanggal_mulai = "not-a-date".to_string(),
                    _ => self.tanggal_jatuh_tempo = "".to_string(),
                };
            }
            "missing_field" => {
                let choice = rng.gen_range(0..4);
                match choice {
                    0 => self.nama_lengkap = "".to_string(),
                    1 => self.jenis_fasilitas = "".to_string(),
                    2 => self.kode_cabang = "".to_string(),
                    _ => self.account_officer = "".to_string(),
                };
            }
            "invalid_currency" => {
                let invalid_currencies = ["XXX", "INVALID", "123", ""];
                self.mata_uang = invalid_currencies[rng.gen_range(0..invalid_currencies.len())].to_string();
            }
            "invalid_collectability" => {
                self.kolektabilitas = rng.gen_range(6..100);
            }
            "future_start_date" => {
                let future = Utc::now() + chrono::Duration::days(rng.gen_range(30..365));
                self.tanggal_mulai = future.format("%Y-%m-%d").to_string();
            }
            "end_before_start" => {
                std::mem::swap(&mut self.tanggal_mulai, &mut self.tanggal_jatuh_tempo);
            }
            "outstanding_gt_plafon" => {
                // violate rule: outstanding <= plafon
                self.saldo_outstanding = self.jumlah_kredit.saturating_add(rng.gen_range(1_000i64..1_000_000i64));
            }
            _ => {
                self.nik = "INVALID".to_string();
            }
        }

        self
    }
}
