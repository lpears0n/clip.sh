//! `rerunsctl`: control the running Reruns app from a terminal or a keyboard
//! shortcut. Bind `rerunsctl save` to a key in your desktop's settings to clip
//! on any desktop.

use std::process::ExitCode;

use clip_ipc::{Request, request};

const USAGE: &str = "usage: rerunsctl <save|screenshot|mark|status|show>

  save        save the last moments of the game being recorded
  screenshot  save the newest frame of the game as a PNG
  mark        mark this moment; saved clips show it on their timeline
  status      print the recorder status as JSON
  show        open the Reruns window";

fn main() -> ExitCode {
    let req = match std::env::args().nth(1).as_deref() {
        Some("save") => Request::Save,
        Some("screenshot") => Request::Screenshot,
        Some("mark") => Request::Mark,
        Some("status") => Request::Status,
        Some("show") => Request::Show,
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match request(&req) {
        Ok(resp) => {
            match (&req, &resp.status) {
                (Request::Status, Some(s)) => {
                    println!("{}", serde_json::to_string_pretty(s).unwrap_or_default())
                }
                _ if resp.ok => println!("{}", resp.message),
                _ => eprintln!("rerunsctl: {}", resp.message),
            }
            if resp.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("rerunsctl: Reruns isn't running ({e})");
            ExitCode::FAILURE
        }
    }
}
