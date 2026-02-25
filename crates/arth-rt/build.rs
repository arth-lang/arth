//! Build script for arth-rt
//!
//! Handles linking to native C libraries based on enabled features.

fn main() {
    // Tell cargo to rerun this build script if Cargo.toml changes
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Link to system libraries based on platform
    #[cfg(target_os = "macos")]
    {
        // Link to System framework for mach_absolute_time
        println!("cargo:rustc-link-lib=System");
    }

    #[cfg(target_os = "linux")]
    {
        // Link to pthread and rt for clock_gettime
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=rt");
    }

    // Feature-based linking

    // SQLite
    #[cfg(feature = "sqlite")]
    {
        // Try to use pkg-config first
        if pkg_config::probe_library("sqlite3").is_err() {
            // Fall back to system library
            println!("cargo:rustc-link-lib=sqlite3");
        }
    }

    // PostgreSQL
    #[cfg(feature = "postgres")]
    {
        // Try to use pg_config to find libpq
        if let Ok(output) = std::process::Command::new("pg_config")
            .arg("--libdir")
            .output()
            && output.status.success()
        {
            let libdir = String::from_utf8_lossy(&output.stdout);
            let libdir = libdir.trim();
            println!("cargo:rustc-link-search=native={}", libdir);
        }
        println!("cargo:rustc-link-lib=pq");
    }

    // OpenSSL (for TLS)
    #[cfg(feature = "tls")]
    {
        // Try to use pkg-config
        if pkg_config::probe_library("openssl").is_err() {
            // Fall back to common locations
            #[cfg(target_os = "macos")]
            {
                // Homebrew OpenSSL location
                println!("cargo:rustc-link-search=native=/opt/homebrew/opt/openssl/lib");
                println!("cargo:rustc-link-search=native=/usr/local/opt/openssl/lib");
            }
            println!("cargo:rustc-link-lib=ssl");
            println!("cargo:rustc-link-lib=crypto");
        }
    }
}
