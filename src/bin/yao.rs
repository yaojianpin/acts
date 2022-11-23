use clap::Parser;
use std::{fs::File, io::Read};
use yao::{Engine, Workflow};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(short, long, value_parser)]
    model: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let workflow = read_file_to_workflow(&args.model);
    let engine = Engine::new();
    engine.push(&workflow);

    let e = engine.clone();
    engine.on_workflow_complete(move |_w: &Workflow| {
        e.close();
    });
    engine.start().await;
}

fn read_file_to_workflow(file_name: &str) -> Workflow {
    let mut file = File::open(file_name).expect("Invalid config file");
    let mut config_value = String::new();
    file.read_to_string(&mut config_value)
        .expect("Read config file error");
    let workflow: Workflow = serde_yaml::from_str(&config_value).expect("Parse yaml error");

    workflow
}
