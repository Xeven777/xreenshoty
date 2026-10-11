//! Read-only inspector for the screenshot database.
//!
//! The system `sqlite3` CLI can read `screenshots` and `screenshots_fts`, but not
//! `screenshot_vec`: vec0 is a static extension linked into this binary, so the
//! CLI has no way to load it and reports "no such module: vec0".

use tauri_native_lib::db;

fn scalar(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(-1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    db::with_conn(|conn| {
        println!("db: {}", db::db_path().unwrap().display());
        println!();
        println!("screenshots        {}", scalar(conn, "SELECT COUNT(*) FROM screenshots"));
        println!(
            "  with OCR text    {}",
            scalar(conn, "SELECT COUNT(*) FROM screenshots WHERE ocr_text != ''")
        );
        println!(
            "  text-free        {}",
            scalar(conn, "SELECT COUNT(*) FROM screenshots WHERE ocr_text = ''")
        );
        println!("  embedded         {}", scalar(conn, "SELECT COUNT(*) FROM screenshots WHERE embedded_at IS NOT NULL"));
        println!();
        println!("screenshots_fts    {} rows", scalar(conn, "SELECT COUNT(*) FROM screenshots_fts"));
        println!("screenshot_vec    {} rows", scalar(conn, "SELECT COUNT(*) FROM screenshot_vec"));
        let schema: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'screenshot_vec'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        println!("  {schema}");
        println!();

        if args.first().map(String::as_str) == Some("--near") {
            // Pure KNN against a vector already in the table: no model needed,
            // so this answers "is the stored space actually meaningful?" without
            // starting llama-server.
            let want = args.get(1).cloned().unwrap_or_default();
            let rowid: i64 = conn
                .query_row(
                    "SELECT id FROM screenshots WHERE path LIKE ?1 ORDER BY id LIMIT 1",
                    [format!("%{want}%")],
                    |r| r.get(0),
                )
                .map_err(|e| format!("no screenshot matching {want:?}: {e}"))?;
            let label: String = conn
                .query_row("SELECT path FROM screenshots WHERE id = ?1", [rowid], |r| r.get(0))
                .unwrap_or_default();

            println!("nearest to {label}\n");
            // vec0 wants the query vector itself, so pull the stored blob out
            // first; a bare rowid is rejected as a non-BLOB.
            let blob: Vec<u8> = conn
                .query_row(
                    "SELECT embedding FROM screenshot_vec WHERE rowid = ?1",
                    [rowid],
                    |r| r.get(0),
                )
                .map_err(|e| format!("no stored vector for rowid {rowid}: {e}"))?;
            // KNN must be planned on the vec0 table directly. Wrapping it in an
            // aliased JOIN hides the `k` constraint from the query planner,
            // which then returns arbitrary rows with distance = 0.
            let mut stmt = conn
                .prepare(
                    "SELECT rowid, distance FROM screenshot_vec
                     WHERE embedding MATCH ?1 AND k = 5 ORDER BY distance",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([blob], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (id, d) = row.map_err(|e| e.to_string())?;
                let path: String = conn
                    .query_row("SELECT path FROM screenshots WHERE id = ?1", [id], |r| r.get(0))
                    .unwrap_or_default();
                let name = path.rsplit('/').next().unwrap_or(&path);
                println!("  {d:.4}  {name}");
            }
        } else if !args.is_empty() {
            println!("usage:\n  dbcheck <words...>      search OCR text (keyword)\n  dbcheck --near <name>    5 nearest screenshots to one (semantic)");
        } else {
            let term = args.join(" ");
            println!("search {term:?} -> fts {}", db::fts_query(&term).unwrap_or("(none)".into()));
            let mut stmt = conn
                .prepare(
                    "SELECT s.path, s.created_at FROM screenshots_fts
                     JOIN screenshots s ON s.id = screenshots_fts.rowid
                     WHERE screenshots_fts MATCH ?1 ORDER BY rank LIMIT 10",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([db::fts_query(&term).unwrap_or_default()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(|e| e.to_string())?;
            let mut n = 0;
            for row in rows {
                let (path, at) = row.map_err(|e| e.to_string())?;
                n += 1;
                println!("  {at}  {path}");
            }
            println!("  ({n} shown, limit 10)");
        }
        Ok(())
    })
    .unwrap_or_else(|e| eprintln!("error: {e}"));
}
