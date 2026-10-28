//! `clipsh`: control the running clip.sh app from a terminal or a keyboard
//! shortcut. Bind `clipsh save` to a key in your desktop's settings to clip
//! on any desktop.

use std::process::ExitCode;

use clip_ipc::{Request, request};

const USAGE: &str = "usage: clipsh <save|status|show>

  save     save the last moments of the game being recorded
  status   print the recorder status as JSON
  show     open the clip.sh window";

fn main() -> ExitCode {
    let req = match std::env::args().nth(1).as_deref() {
        Some("save") => Request::Save,
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
                _ => eprintln!("clipsh: {}", resp.message),
            }
            if resp.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("clipsh: clip.sh isn't running ({e})");
            ExitCode::FAILURE
        }
    }
}
