//! Loads every JSFX under a folder and reports what fails and why.
//! `cargo run -p soundcraft-jsfx-host --example corpus -- [effects-dir]`

use soundcraft_dsp::Plugin;
use soundcraft_jsfx_host::{JsfxPlugin, scan};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let root = std::env::args_os().nth(1).map(PathBuf::from).or_else(|| scan::default_effects_dirs().into_iter().next());
    let Some(root) = root else {
        eprintln!("no Effects folder found; pass one as the argument");
        return;
    };
    let found = scan::scan(&root);
    let mut ok = 0usize;
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &found {
        match JsfxPlugin::from_file(&root, &e.path) {
            Ok(mut p) => {
                p.prepare(48_000.0, 256, 2);
                let mut io = vec![vec![0.1f32; 256], vec![-0.1f32; 256]];
                p.process(&mut io, 256);
                if p.is_faulted() {
                    reasons.entry("runtime: ran out of budget".into()).or_default().push(e.id.clone());
                } else {
                    ok += 1;
                }
                let _ = p.info();
            }
            Err(err) => reasons.entry(err.to_string()).or_default().push(e.id.clone()),
        }
    }
    println!("{} effects found in {}: {ok} load and run, {} fail", found.len(), root.display(), found.len() - ok);
    let mut sorted: Vec<_> = reasons.into_iter().collect();
    sorted.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (reason, ids) in sorted.iter().take(40) {
        println!("{:>4}  {reason}   e.g. {}", ids.len(), ids.first().map_or("", String::as_str));
    }
}
