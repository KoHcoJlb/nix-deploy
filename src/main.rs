use nix_deploy::{run, terminal::print_error};

fn main() {
    if let Err(err) = run() {
        print_error(err);
    }
}
