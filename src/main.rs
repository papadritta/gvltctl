use bip32::{Mnemonic, Prefix, XPrv};
use cargo_metadata::Metadata;
use clap::ArgAction;
use clap::{value_parser, Arg, Command, ValueHint};
use clap_complete::{generate, Shell};
use cosmrs::crypto::secp256k1::SigningKey;
use gevulot_rs::gevulot_client::GevulotClientBuilder;
use gevulot_rs::GevulotClient;
use rand_core::OsRng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};

#[cfg(target_os = "linux")]
mod builders;
mod commands;

#[cfg(target_os = "linux")]
use commands::build::*;
use commands::{pins::*, sudo::*, tasks::*, workers::*};

shadow_rs::shadow!(build_info);

/// Main entry point for the Gevulot Control CLI application.
///
/// This function sets up the command-line interface, parses arguments,
/// and dispatches to the appropriate subcommand handlers.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Parse command-line arguments
    let cmd = setup_command_line_args()?;

    // Handle matches here
    match cmd.get_matches().subcommand() {
        Some(("worker", sub_m)) => match sub_m.subcommand() {
            Some(("list", sub_m)) => list_workers(sub_m).await?,
            Some(("get", sub_m)) => get_worker(sub_m).await?,
            Some(("create", sub_m)) => create_worker(sub_m).await?,
            Some(("delete", sub_m)) => delete_worker(sub_m).await?,
            _ => println!("Unknown worker command"),
        },
        Some(("pin", sub_m)) => match sub_m.subcommand() {
            Some(("list", sub_m)) => list_pins(sub_m).await?,
            Some(("get", sub_m)) => get_pin(sub_m).await?,
            Some(("create", sub_m)) => create_pin(sub_m).await?,
            Some(("delete", sub_m)) => delete_pin(sub_m).await?,
            Some(("ack", sub_m)) => ack_pin(sub_m).await?,
            _ => println!("Unknown pin command"),
        },
        Some(("task", sub_m)) => match sub_m.subcommand() {
            Some(("list", sub_m)) => list_tasks(sub_m).await?,
            Some(("get", sub_m)) => get_task(sub_m).await?,
            Some(("create", sub_m)) => create_task(sub_m).await?,
            Some(("accept", sub_m)) => accept_task(sub_m).await?,
            Some(("decline", sub_m)) => decline_task(sub_m).await?,
            Some(("finish", sub_m)) => finish_task(sub_m).await?,
            _ => println!("Unknown task command"),
        },
        Some(("workflow", sub_m)) => match sub_m.subcommand() {
            Some(("list", sub_m)) => list_workflows(sub_m).await?,
            Some(("get", sub_m)) => get_workflow(sub_m).await?,
            Some(("create", sub_m)) => create_workflow(sub_m).await?,
            Some(("delete", sub_m)) => delete_workflow(sub_m).await?,
            _ => println!("Unknown workflow command"),
        },
        Some(("sudo", sub_m)) => match sub_m.subcommand() {
            Some(("delete-pin", sub_m)) => sudo_delete_pin(sub_m).await?,
            Some(("delete-worker", sub_m)) => sudo_delete_worker(sub_m).await?,
            Some(("delete-task", sub_m)) => sudo_delete_task(sub_m).await?,
            Some(("freeze-account", sub_m)) => sudo_freeze_account(sub_m).await?,
            _ => println!("Unknown sudo command"),
        },
        Some(("keygen", sub_m)) => generate_key(sub_m).await?,
        Some(("compute-key", sub_m)) => compute_key(sub_m).await?,
        Some(("send", sub_m)) => send_tokens(sub_m).await?,
        Some(("account-info", sub_m)) => account_info(sub_m).await?,
        Some(("generate-completion", sub_m)) => generate_completion(sub_m).await?,
        #[cfg(target_os = "linux")]
        Some(("build", sub_m)) => build(sub_m).await?,
        _ => println!("Unknown command"),
    }

    Ok(())
}

/// Get gevulot-rs dependency version from cargo metadata.
fn get_gevulot_rs_version(metadata: &Metadata) -> Option<String> {
    const GEVULOT_RS_NAME: &str = "gevulot-rs";
    let gvltctl = metadata.root_package()?;
    let gevulot_rs_dep = gvltctl
        .dependencies
        .iter()
        .find(|dep| &dep.name == GEVULOT_RS_NAME)?;

    if let Some(path) = gevulot_rs_dep.path.as_ref() {
        Some(format!(
            "{} ({})",
            metadata
                .packages
                .iter()
                .find(|package| {
                    &package.name == GEVULOT_RS_NAME && package.id.repr.starts_with("path")
                })?
                .version,
            path.as_str()
        ))
    } else if gevulot_rs_dep
        .source
        .as_ref()
        .is_some_and(|src| src.starts_with("git"))
    {
        metadata.packages.iter().find_map(|package| {
            if &package.name == GEVULOT_RS_NAME {
                package
                    .id
                    .repr
                    .strip_prefix("git+")?
                    .split('#')
                    .collect::<Vec<_>>()
                    .get(0)
                    .map(|id| format!("{} ({})", package.version, id))
            } else {
                None
            }
        })
    } else if gevulot_rs_dep
        .source
        .as_ref()
        .is_some_and(|src| src.starts_with("registry"))
    {
        metadata.packages.iter().find_map(|package| {
            if &package.name == GEVULOT_RS_NAME
                && package
                    .source
                    .as_ref()
                    .is_some_and(cargo_metadata::Source::is_crates_io)
            {
                Some(package.version.to_string())
            } else {
                None
            }
        })
    } else {
        return None;
    }
}

/// Parses command-line arguments and returns the matches.
///
/// This function sets up the entire command-line interface structure,
/// including all subcommands and their respective arguments.
fn setup_command_line_args() -> Result<Command, Box<dyn std::error::Error>> {
    let chain_args: [Arg; 6] = [
        Arg::new("endpoint")
            .short('e')
            .long("endpoint")
            .value_name("URL")
            .env("GEVULOT_ENDPOINT")
            .help("Sets the endpoint for the Gevulot client")
            .value_hint(ValueHint::Url)
            .action(ArgAction::Set),
        Arg::new("gas_price")
            .short('g')
            .long("gas-price")
            .value_name("PRICE")
            .env("GEVULOT_GAS_PRICE")
            .help("Sets the gas price for the Gevulot client")
            .value_hint(ValueHint::Other)
            .action(ArgAction::Set),
        Arg::new("gas_multiplier")
            .short('m')
            .long("gas-multiplier")
            .value_name("MULTIPLIER")
            .env("GEVULOT_GAS_MULTIPLIER")
            .help("Sets the gas multiplier for the Gevulot client")
            .value_hint(ValueHint::Other)
            .action(ArgAction::Set),
        Arg::new("mnemonic")
            .short('n')
            .long("mnemonic")
            .value_name("MNEMONIC")
            .env("GEVULOT_MNEMONIC")
            .help("Sets the mnemonic for the Gevulot client")
            .value_hint(ValueHint::Other)
            .action(ArgAction::Set),
        Arg::new("password")
            .short('n')
            .long("password")
            .value_name("PASSWORD")
            .env("GEVULOT_PASSWORD")
            .help("Sets the password for the Gevulot client")
            .value_hint(ValueHint::Other)
            .action(ArgAction::Set),
        Arg::new("format")
            .short('F')
            .long("format")
            .value_name("FORMAT")
            .env("GEVULOT_FORMAT")
            .help("Sets the output format (yaml, json, prettyjson, toml)")
            .value_hint(ValueHint::Other)
            .default_value("yaml")
            .action(ArgAction::Set),
    ];

    let gevulot_rs_version =
        serde_json::from_slice::<serde_json::Value>(&build_info::CARGO_METADATA)
            .ok()
            .map(Metadata::deserialize)
            .map(Result::ok)
            .flatten()
            .as_ref()
            .map(get_gevulot_rs_version)
            .flatten();

    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut command = clap::command!()
        .long_version(format!(
            "{} ({})\ngevulot-rs {}\nplatform: {}",
            build_info::PKG_VERSION,
            if build_info::GIT_CLEAN {
                format!(
                    "{} {}",
                    if build_info::TAG.is_empty() {
                        build_info::SHORT_COMMIT
                    } else {
                        build_info::TAG
                    },
                    // Strip commit time and leave only date
                    build_info::COMMIT_DATE.split(' ').collect::<Vec<_>>()[0],
                )
            } else {
                format!("{}-dirty", build_info::SHORT_COMMIT)
            },
            gevulot_rs_version.unwrap_or_else(|| "unknown".to_string()),
            build_info::BUILD_TARGET,
        ))
        .subcommand_required(true)
        // Worker subcommand
        .subcommand(
            Command::new("worker")
                .about("Commands related to workers")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List all workers")
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("get")
                        .about("Get a specific worker")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the worker to retrieve")
                                .required(true)
                                .index(1),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("create")
                        .about("Create a new worker")
                        .arg(
                            Arg::new("file")
                                .short('f')
                                .long("file")
                                .value_name("FILE")
                                .value_hint(ValueHint::FilePath)
                                .help("The file to read the worker data from, defaults to stdin")
                                .action(ArgAction::Set),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete a worker")
                        .args(&chain_args),
                ),
        )
        // Pin subcommand
        .subcommand(
            Command::new("pin")
                .about("Commands related to pins")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List all pins")
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("get")
                        .about("Get a specific pin")
                        .arg(
                            Arg::new("cid")
                                .value_name("CID")
                                .help("The CID of the pin to retrieve")
                                .value_hint(ValueHint::Other)
                                .required(true)
                                .index(1),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("ack")
                        .about("Ack a pin")
                        .arg(
                            Arg::new("cid")
                                .value_name("CID")
                                .help("The CID of the pin to ack")
                                .value_hint(ValueHint::Other)
                                .required(true)
                                .index(1),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("create")
                        .about("Create a new pin")
                        .arg(
                            Arg::new("file")
                                .short('f')
                                .long("file")
                                .value_name("FILE")
                                .value_hint(ValueHint::FilePath)
                                .help("The file to read the pin data from, defaults to stdin")
                                .action(ArgAction::Set),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete a pin")
                        .args(&chain_args),
                ),
        )
        // Task subcommand
        .subcommand(
            Command::new("task")
                .about("Commands related to tasks")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List all tasks")
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("get")
                        .about("Get a specific task")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the task to retrieve")
                                .value_hint(ValueHint::Other)
                                .required(true)
                                .index(1),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("create")
                        .about("Create a new task")
                        .arg(
                            Arg::new("file")
                                .short('f')
                                .long("file")
                                .value_name("FILE")
                                .help("The file to read the task data from, defaults to stdin")
                                .value_hint(ValueHint::FilePath)
                                .action(ArgAction::Set),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("accept")
                        .about("Accept a task (you probably should not use this)")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the task to accept")
                                .required(true)
                                .index(1),
                        )
                        .arg(
                            Arg::new("worker_id")
                                .value_name("WORKER_ID")
                                .help("The ID of the worker accepting the task")
                                .required(true)
                                .index(2),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("decline")
                        .about("Decline a task (you probably should not use this)")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the task to decline")
                                .required(true)
                                .index(1),
                        )
                        .arg(
                            Arg::new("worker_id")
                                .value_name("WORKER_ID")
                                .help("The ID of the worker declining the task")
                                .required(true)
                                .index(2),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("finish")
                        .about("Finish a task (you probably should not use this)")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the task to finish")
                                .required(true)
                                .index(1),
                        )
                        .arg(
                            Arg::new("exit_code")
                                .value_name("EXIT_CODE")
                                .help("The exit code of the task")
                                .value_parser(value_parser!(i32))
                                .required(false),
                        )
                        .arg(
                            Arg::new("stdout")
                                .value_name("STDOUT")
                                .help("The stdout output of the task")
                                .required(false),
                        )
                        .arg(
                            Arg::new("stderr")
                                .value_name("STDERR")
                                .help("The stderr output of the task")
                                .required(false),
                        )
                        .arg(
                            Arg::new("error")
                                .value_name("ERROR")
                                .help("Any error message from the task")
                                .required(false),
                        )
                        .arg(
                            Arg::new("output_contexts")
                                .value_name("OUTPUT_CONTEXTS")
                                .help("Output contexts produced by the task")
                                .required(false)
                                .action(ArgAction::Append),
                        )
                        .args(&chain_args),
                ),
        )
        // Workflow subcommand
        .subcommand(
            Command::new("workflow")
                .about("Commands related to workflows")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List all workflows")
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("get")
                        .about("Get a specific workflow")
                        .arg(
                            Arg::new("id")
                                .value_name("ID")
                                .help("The ID of the workflow to retrieve")
                                .value_hint(ValueHint::Other)
                                .required(true)
                                .index(1),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("create")
                        .about("Create a new workflow")
                        .arg(
                            Arg::new("file")
                                .short('f')
                                .long("file")
                                .value_name("FILE")
                                .help("The file to read the workflow data from, defaults to stdin")
                                .value_hint(ValueHint::FilePath)
                                .action(ArgAction::Set),
                        )
                        .args(&chain_args),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete a workflow")
                        .args(&chain_args),
                ),
        )
        // Keygen subcommand
        .subcommand(
            Command::new("keygen")
                .about("Generate a new key")
                .arg(
                    Arg::new("file")
                        .short('f')
                        .long("file")
                        .value_name("FILE")
                        .help("The file to write the seed to, defaults to stdout")
                        .value_hint(ValueHint::FilePath)
                        .action(ArgAction::Set),
                )
                .arg(
                    Arg::new("password")
                        .short('p')
                        .long("password")
                        .value_name("PASSWORD")
                        .help("Sets the password for the Gevulot client")
                        .value_hint(ValueHint::Other)
                        .action(ArgAction::Set)
                        .global(true),
                )
                .arg(
                    Arg::new("format")
                        .short('F')
                        .long("format")
                        .value_name("FORMAT")
                        .default_value("yaml")
                        .help("Sets the output format (yaml, json, prettyjson, toml)"),
                ),
        )
        .subcommand(
            Command::new("compute-key")
                .about("Compute a key")
                .arg(
                    Arg::new("mnemonic")
                        .long("mnemonic")
                        .value_name("MNEMONIC")
                        .env("GEVULOT_MNEMONIC")
                        .help("The mnemonic to compute the key from")
                        .required(true)
                        .value_hint(ValueHint::Other),
                )
                .arg(
                    Arg::new("password")
                        .long("password")
                        .value_name("PASSWORD")
                        .env("GEVULOT_PASSWORD")
                        .help("The password to compute the key with")
                        .value_hint(ValueHint::Other),
                )
                .arg(
                    Arg::new("format")
                        .short('F')
                        .long("format")
                        .value_name("FORMAT")
                        .default_value("yaml")
                        .help("Sets the output format (yaml, json, prettyjson, toml"),
                ),
        )
        // Send subcommand
        .subcommand(
            Command::new("send")
                .about("Send tokens to a receiver on the Gevulot network")
                .arg(
                    Arg::new("amount")
                        .value_name("AMOUNT")
                        .help("The amount of tokens to send")
                        .required(true)
                        .index(1)
                        .value_hint(ValueHint::Other),
                )
                .arg(
                    Arg::new("receiver")
                        .value_name("RECEIVER")
                        .help("The receiver address")
                        .required(true)
                        .index(2)
                        .value_hint(ValueHint::Other),
                )
                .args(&chain_args),
        )
        // Account-info subcommand
        .subcommand(
            Command::new("account-info")
                .about("Get the balance of the given account")
                .arg(
                    Arg::new("address")
                        .value_name("ADDRESS")
                        .help("The address to get the balance of")
                        .required(true)
                        .index(1)
                        .value_hint(ValueHint::Other),
                )
                .args(&chain_args),
        )
        .subcommand(
            Command::new("generate-completion")
                .about("Generate shell completion scripts")
                .arg(
                    Arg::new("shell")
                        .value_name("SHELL")
                        .help("The shell to generate the completion scripts for")
                        .required(true)
                        .action(ArgAction::Set)
                        .value_parser(value_parser!(clap_complete::Shell))
                        .index(1)
                        .value_hint(ValueHint::Other),
                )
                .arg(
                    Arg::new("file")
                        .short('f')
                        .long("file")
                        .value_name("FILE")
                        .help("The file to write the completion scripts to, defaults to stdout")
                        .action(ArgAction::Set)
                        .value_hint(ValueHint::FilePath),
                ),
        )
        .subcommand(commands::sudo::get_command(&chain_args));

    #[cfg(target_os = "linux")]
    {
        command = command.subcommand(commands::build::get_command());
    }

    Ok(command)
}

/// Connects to the Gevulot network using the provided command-line arguments.
///
/// This function creates a GevulotClient based on the endpoint, gas price,
/// gas multiplier, and mnemonic provided in the command-line arguments.
///
/// # Arguments
///
/// * `matches` - A reference to the ArgMatches struct containing parsed command-line arguments.
///
/// # Returns
///
/// A Result containing a GevulotClient if successful, or a Box<dyn std::error::Error> if an error occurs.
async fn connect_to_gevulot(
    matches: &clap::ArgMatches,
) -> Result<GevulotClient, Box<dyn std::error::Error>> {
    let mut client_builder = GevulotClientBuilder::default();

    // Set the endpoint if provided
    if let Some(endpoint) = matches.get_one::<String>("endpoint") {
        client_builder = client_builder.endpoint(endpoint);
    }

    // Set the gas price if provided
    if let Some(gas_price) = matches.get_one::<String>("gas_price") {
        client_builder = client_builder.gas_price(
            gas_price
                .parse()
                .map_err(|e| format!("Failed to parse gas_price: {}", e))?,
        );
    }

    // Set the gas multiplier if provided
    if let Some(gas_multiplier) = matches.get_one::<String>("gas_multiplier") {
        client_builder = client_builder.gas_multiplier(
            gas_multiplier
                .parse()
                .map_err(|e| format!("Failed to parse gas_multiplier: {}", e))?,
        );
    }

    // Set the mnemonic if provided
    if let Some(mnemonic) = matches.get_one::<String>("mnemonic") {
        client_builder = client_builder.mnemonic(mnemonic);
    }

    // Set the password if provided
    if let Some(password) = matches.get_one::<String>("password") {
        client_builder = client_builder.password(password);
    }

    // Build and return the client
    let client = client_builder.build().await?;

    Ok(client)
}

/// Reads and parses a file or stdin input into a specified type.
///
/// This function is generic over T, which must implement DeserializeOwned.
/// It reads from a file if specified in the command-line arguments,
/// otherwise it reads from stdin.
///
/// # Arguments
///
/// * `matches` - A reference to the ArgMatches struct containing parsed command-line arguments.
///
/// # Returns
///
/// A Result containing the parsed value of type T if successful, or a Box<dyn std::error::Error> if an error occurs.
async fn read_file<T: DeserializeOwned>(
    matches: &clap::ArgMatches,
) -> Result<T, Box<dyn std::error::Error>> {
    let content = match matches.get_one::<String>("file") {
        Some(file) => {
            let mut file = File::open(file)?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            contents
        }
        None => {
            let mut contents = String::new();
            io::stdin().read_to_string(&mut contents)?;
            contents
        }
    };
    let parsed: T = serde_yaml::from_str(&content)?;
    Ok(parsed)
}

/// Prints an object in the specified format.
///
/// This function takes a reference to command-line arguments and a serializable value,
/// and prints the value in the format specified by the user (yaml, json, prettyjson, or toml).
///
/// # Arguments
///
/// * `matches` - A reference to the ArgMatches struct containing parsed command-line arguments.
/// * `value` - A reference to the value to be printed, which must implement Serialize.
///
/// # Returns
///
/// A Result indicating success or an error if serialization or printing fails.
fn print_object<T: Serialize>(
    matches: &clap::ArgMatches,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get the format from command-line arguments, defaulting to "yaml" if not specified
    let format = matches
        .get_one::<String>("format")
        .expect("format has a default value");

    // Match on the format string and serialize/print accordingly
    match format.as_str() {
        "yaml" => {
            // Serialize to YAML and print
            let yaml = serde_yaml::to_string(value)?;
            println!("{}", yaml);
        }
        "json" => {
            // Serialize to compact JSON and print
            let json = serde_json::to_string(value)?;
            println!("{}", json);
        }
        "prettyjson" => {
            // Serialize to pretty-printed JSON and print
            let prettyjson = serde_json::to_string_pretty(value)?;
            println!("{}", prettyjson);
        }
        "toml" => {
            // Serialize to TOML and print
            let toml = toml::to_string(value)?;
            println!("{}", toml);
        }
        // If an unknown format is specified, print an error message
        _ => println!("Unknown format"),
    }

    Ok(())
}

/// Sends tokens to a receiver on the Gevulot network.
///
/// # Arguments
///
/// * `sub_m` - A reference to the ArgMatches struct containing parsed command-line arguments.
///
/// # Returns
///
/// A Result indicating success or an error if the token transfer fails.
async fn send_tokens(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let client = connect_to_gevulot(_sub_m).await?;
    let amount = _sub_m.get_one::<String>("amount").unwrap();
    let receiver = _sub_m.get_one::<String>("receiver").unwrap();
    client
        .base_client
        .write()
        .await
        .token_transfer(receiver, amount.parse()?)
        .await?;

    let output = serde_json::json!({
        "success": true,
        "amount": amount,
        "receiver": receiver
    });

    print_object(_sub_m, &output)?;

    Ok(())
}

/// Retrieves and displays account information for a given address.
///
/// # Arguments
///
/// * `sub_m` - A reference to the ArgMatches struct containing parsed command-line arguments.
///https://github.com/gevulotnetwork/platform/pull/60
/// # Returns
///
/// A Result indicating success or an error if the account information retrieval fails.
async fn account_info(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let client = connect_to_gevulot(_sub_m).await?;
    let address = _sub_m.get_one::<String>("address").unwrap();
    let account = client
        .base_client
        .write()
        .await
        .get_account(address)
        .await?;
    let balance = client
        .base_client
        .write()
        .await
        .get_account_balance(address)
        .await?;

    let output = serde_json::json!({
        "account_number": account.account_number,
        "sequence": account.sequence,
        "balance": balance.amount.to_string()
    });

    print_object(_sub_m, &output)?;
    Ok(())
}

/// Generates a new key and optionally saves it to a file.
async fn generate_key(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    // Generate random Mnemonic using the default language (English)
    let mnemonic = Mnemonic::random(OsRng, Default::default());
    let password = _sub_m
        .get_one::<String>("password")
        .cloned()
        .unwrap_or("".to_string());

    // Derive a BIP39 seed value using the given password
    let seed = mnemonic.to_seed(&password);

    // Derive a child `XPrv` using the provided BIP32 derivation path
    let child_path = "m/44'/118'/0'/0/0";
    let child_xprv = XPrv::derive_from_path(&seed, &child_path.parse()?)?;

    // Get the `XPub` associated with `child_xprv`.
    let child_xpub = child_xprv.public_key();

    // Serialize `child_xprv` as a string with the `xprv` prefix.
    let child_xprv_str = child_xprv.to_string(Prefix::XPRV);
    assert!(child_xprv_str.starts_with("xprv"));

    // Serialize `child_xpub` as a string with the `xpub` prefix.
    let child_xpub_str = child_xpub.to_string(Prefix::XPUB);
    assert!(child_xpub_str.starts_with("xpub"));

    // Get the ECDSA/secp256k1 signing and verification keys for the xprv and xpub
    let sk = SigningKey::from_slice(&child_xprv.private_key().to_bytes())?;

    let account_id = sk.public_key().account_id("gvlt").unwrap();
    let phrase = mnemonic.phrase();

    let output = serde_json::json!({
        "account_id": account_id,
        "mnemonic": phrase
    });

    if let Some(file) = _sub_m.get_one::<String>("file") {
        let mut file = File::create(file)?;
        file.write_all(phrase.as_bytes())?;
    }

    match _sub_m.get_one::<String>("format").map(String::as_str) {
        Some("json") => {
            let json = serde_json::to_string(&output)?;
            println!("{}", json);
        }
        Some("prettyjson") => {
            let prettyjson = serde_json::to_string_pretty(&output)?;
            println!("{}", prettyjson);
        }
        Some("toml") => {
            let toml = toml::to_string(&output)?;
            println!("{}", toml);
        }
        Some("yaml") => {
            let yaml = serde_yaml::to_string(&output)?;
            println!("{}", yaml);
        }
        _ => {
            println!("{}", account_id);
            println!("{}", phrase);
        }
    }

    Ok(())
}

/// Generates a new key and optionally saves it to a file.
async fn compute_key(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let mnemonic = Mnemonic::new(
        _sub_m.get_one::<String>("mnemonic").unwrap(),
        bip32::Language::English,
    )?;

    let password = _sub_m
        .get_one::<String>("password")
        .cloned()
        .unwrap_or("".to_string());

    // Derive a BIP39 seed value using the given password
    let seed = mnemonic.to_seed(&password);

    // Derive a child `XPrv` using the provided BIP32 derivation path
    let child_path = "m/44'/118'/0'/0/0";
    let child_xprv = XPrv::derive_from_path(&seed, &child_path.parse()?)?;

    // Get the `XPub` associated with `child_xprv`.
    let child_xpub = child_xprv.public_key();

    // Serialize `child_xprv` as a string with the `xprv` prefix.
    let child_xprv_str = child_xprv.to_string(Prefix::XPRV);
    assert!(child_xprv_str.starts_with("xprv"));

    // Serialize `child_xpub` as a string with the `xpub` prefix.
    let child_xpub_str = child_xpub.to_string(Prefix::XPUB);
    assert!(child_xpub_str.starts_with("xpub"));

    // Get the ECDSA/secp256k1 signing and verification keys for the xprv and xpub
    let sk = SigningKey::from_slice(&child_xprv.private_key().to_bytes())?;

    let account_id = sk.public_key().account_id("gvlt").unwrap();

    let output = serde_json::json!({ "account_id": account_id });
    print_object(_sub_m, &output)?;
    Ok(())
}

/// Generates shell completion scripts for the gvltctl command-line tool.
///
/// This function generates shell completion scripts for the gvltctl command-line tool
/// and prints the results to the console.
///
/// # Arguments
///
/// * `sub_m` - A reference to the ArgMatches struct containing parsed command-line arguments.
///
/// # Returns
///
/// A Result indicating success or an error if the completion generation fails.
async fn generate_completion(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(generator) = _sub_m.get_one::<Shell>("shell").copied() {
        let mut cmd = setup_command_line_args()?; // Assuming you have a Command::new() function
        eprintln!("Generating completion file for {generator}...");
        if let Some(file) = _sub_m.get_one::<String>("file") {
            let mut file = File::create(file)?;
            generate(generator, &mut cmd, "gvltctl", &mut file);
        } else {
            generate(generator, &mut cmd, "gvltctl", &mut io::stdout());
        }
    } else {
        eprintln!("No shell specified for completion generation");
    }
    Ok(())
}
async fn list_workflows(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let output = serde_json::json!({
        "message": "Listing all workflows",
        "status": "not_implemented"
    });
    print_object(_sub_m, &output)?;
    todo!();
}

async fn get_workflow(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    println!("Getting a specific workflow");
    todo!();
}

async fn create_workflow(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating a new workflow");
    todo!();
}

async fn delete_workflow(_sub_m: &clap::ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    println!("Deleting a workflow");
    todo!();
}
