use clap::Parser;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use credit_data_simulator::models::credit::CreditRecord;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

/// High-performance SLIK credit data generator for NDJSON
#[derive(Parser, Debug)]
#[command(author, version, about = "High-performance SLIK credit data generator")]
struct Args {
    /// Number of records to generate
    #[arg(short, long, default_value_t = 100)]
    count: u32,

    /// Output file path (NDJSON)
    #[arg(short, long)]
    output: Option<String>,

    /// Ratio of records with injected errors (0.0 to 1.0)
    #[arg(short, long, default_value_t = 0.0)]
    dirty_ratio: f64,

    /// Random seed
    #[arg(short, long)]
    seed: Option<u64>,

    /// Specific file to load (skips generation if provided)
    #[arg(short, long)]
    file: Option<String>,

    /// Simulator URL to load data directly (e.g. http://localhost:18081)
    #[arg(short, long)]
    load_to: Option<String>,
    
    /// Append to existing records in simulator
    #[arg(short, long, default_value_t = false)]
    append: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let start = Instant::now();

    let output_path = if let Some(ref f) = args.file {
        println!("📂 Using existing file: {}", f);
        f.clone()
    } else {
        println!("🚀 Starting generation of {} records...", args.count);

        let mut rng = if let Some(s) = args.seed {
            ChaCha8Rng::seed_from_u64(s)
        } else {
            ChaCha8Rng::from_rng(rand::thread_rng())?
        };

        let error_types = [
            "invalid_nik",
            "negative_amount",
            "invalid_date",
            "missing_field",
            "invalid_currency",
            "invalid_collectability",
            "outstanding_gt_plafon",
        ];

        let error_threshold = (args.dirty_ratio.clamp(0.0, 1.0) * u64::MAX as f64) as u64;

        // Use a default filename if no output file specified
        let path = args.output.clone().unwrap_or_else(|| {
            format!("credits_{}records_dirty{}_{}.ndjson", 
                args.count, 
                args.dirty_ratio, 
                chrono::Local::now().format("%Y%m%d_%H%M%S"))
        });

        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);

        for i in 0..args.count {
            let mut record = CreditRecord::generate_clean(&mut rng, i as u64);

            if error_threshold > 0 && rng.gen::<u64>() < error_threshold {
                let error_type = error_types[rng.gen_range(0..error_types.len())];
                record = record.inject_error(&mut rng, error_type);
            }

            let json = serde_json::to_string(&record)?;
            writeln!(writer, "{}", json)?;

            if (i + 1) % 100000 == 0 {
                println!("   ... generated {} records ({:?})", i + 1, start.elapsed());
            }
        }

        writer.flush()?;
        println!("✅ Generation complete! File saved to: {}", path);
        path
    };

    println!("⏱️ Elapsed Time (Generation/Check): {:?}", start.elapsed());

    if let Some(url) = args.load_to {
        println!("📤 Uploading to simulator at {}...", url);
        let upload_start = Instant::now();
        
        let client = reqwest::Client::new();
        // Set a long timeout for large uploads
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()?;

        let file = tokio::fs::File::open(&output_path).await?;
        let stream = tokio_util::io::ReaderStream::new(file);
        let body = reqwest::Body::wrap_stream(stream);

        let endpoint = format!("{}/api/v1/credits/load-ndjson?append={}", url, args.append);
        let res = client.post(endpoint)
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await?;

        if res.status().is_success() {
            println!("✅ Upload successful! ({:?})", upload_start.elapsed());
            let json: serde_json::Value = res.json().await?;
            println!("📊 Response: {}", serde_json::to_string_pretty(&json)?);
        } else {
            let status = res.status();
            let text = res.text().await?;
            println!("❌ Upload failed: {}", status);
            println!("📜 Body: {}", text);
        }
    }

    Ok(())
}
