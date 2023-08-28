use std::sync::atomic::{AtomicU64, Ordering};

use clap::Parser;
use eyre::{ContextCompat, Result, WrapErr};
use tracing::{debug, info, instrument, warn};

use canvas_trello_sync as lib;

#[derive(clap::Parser)]
struct Args {
    /// Increase logging verbosity
    #[clap(short, long)]
    verbose: bool,

    /// Path to the configuration file.
    #[clap(short, long, default_value = "config.toml")]
    config: std::path::PathBuf,

    /// Canvas access token
    #[clap(long, env = "CANVAS_ACCESS_TOKEN")]
    canvas_access_token: String,

    /// Trello API key
    #[clap(long, env = "TRELLO_API_KEY")]
    trello_api_key: String,

    /// Trello API Secret
    #[clap(long, env = "TRELLO_API_TOKEN")]
    trello_api_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Configure tracing to show events, and to show info-level spans.
    let default_verbosity = if args.verbose {
        tracing_subscriber::filter::LevelFilter::DEBUG
    } else {
        tracing_subscriber::filter::LevelFilter::INFO
    };
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(default_verbosity.into())
        .from_env_lossy();
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_span_events(
            tracing_subscriber::fmt::format::FmtSpan::CLOSE
                | tracing_subscriber::fmt::format::FmtSpan::NEW,
        )
        .with_target(false)
        .init();

    // Load the config file
    let config_bytes = tokio::fs::read_to_string(&args.config)
        .await
        .wrap_err_with(|| format!("Failed to read config file {:?}", args.config))?;
    let config: lib::config::Config =
        toml::from_str(&config_bytes).wrap_err("Failed to parse config file")?;

    // Create API clients.
    let canvas = lib::canvas::Client::from_url_and_token(
        &config.canvas.graphql_endpoint,
        &args.canvas_access_token,
    );
    let trello =
        lib::trello::Client::from_key_and_token(&args.trello_api_key, &args.trello_api_token);

    // Get the current state of the Trello board.
    let current_board = {
        let span = tracing::info_span!("Get board contents", board_id = %config.trello.board_id);
        let _guard = span.enter();

        trello
            .get_board_contents(&config.trello.board_id)
            .await
            .wrap_err("Failed to get board contents")?
    };

    // Get the ID of the `Canvas URL` custom field.
    let canvas_url_field_id = {
        current_board
            .custom_fields
            .iter()
            .find(|f| f.name == "Canvas URL")
            .wrap_err("No custom field found named 'Canvas URL'")?
            .id
            .clone()
    };
    info!(%canvas_url_field_id);

    // Get the ID of the rightmost list.
    let new_card_list_id = {
        current_board
            .lists
            .iter()
            .find(|l| l.name == config.trello.add_to_list)
            .wrap_err_with(|| format!("Could not find list {:?}", config.trello.add_to_list))?
            .id
            .clone()
    };

    // Create the context.
    let ctx = Context {
        trello,
        current_board,
        canvas_url_field_id,
        new_card_list_id,

        canvas,
        config,

        count_assignments: AtomicU64::new(0),
        count_created: AtomicU64::new(0),
        count_updated: AtomicU64::new(0),
        count_up_to_date: AtomicU64::new(0),
    };

    // Loop through the mappings, syncing each one.
    for mapping in &ctx.config.mapping {
        sync_mapping(&ctx, mapping)
            .await
            .wrap_err_with(|| format!("Failed to sync mapping: {:?}", mapping.trello_label_name))?;
    }

    // Summarize
    info!(
        assignments = ctx.count_assignments.load(Ordering::Relaxed),
        created = ctx.count_created.load(Ordering::Relaxed),
        updated = ctx.count_updated.load(Ordering::Relaxed),
        up_to_date = ctx.count_up_to_date.load(Ordering::Relaxed),
        "Sync Complete",
    );

    Ok(())
}

struct Context {
    trello: lib::trello::Client,
    current_board: lib::trello::Board,
    canvas_url_field_id: String,
    new_card_list_id: String,

    canvas: lib::canvas::Client,
    config: lib::config::Config,

    count_assignments: AtomicU64,
    count_created: AtomicU64,
    count_updated: AtomicU64,
    count_up_to_date: AtomicU64,
}

#[instrument(level = "INFO", skip_all, fields(course = %mapping.trello_label_name))]
async fn sync_mapping(ctx: &Context, mapping: &lib::config::Mapping) -> Result<()> {
    // Fetch the assignment list from the course.
    let assignments = ctx
        .canvas
        .get_assignments(&mapping.canvas_course_id)
        .await
        .wrap_err("Failed to fetch assignment list")?;

    // Loop through the assignments, syncing each one.
    for assignment in assignments {
        ctx.count_assignments.fetch_add(1, Ordering::Relaxed);
        sync_assignment(ctx, mapping, &assignment)
            .await
            .wrap_err_with(|| format!("Failed to sync assignment: {:?}", assignment.name))?;
    }

    Ok(())
}

#[instrument(level = "INFO", skip_all, fields(name = %assignment.name))]
async fn sync_assignment(
    ctx: &Context,
    mapping: &lib::config::Mapping,
    assignment: &lib::canvas::Assignment,
) -> Result<()> {
    // Format the `Canvas URL` field for this assignment:
    let mut canvas_url = assignment.html_url.clone();
    canvas_url.set_scheme("https").unwrap();
    info!(%canvas_url);

    // Get the right label ID for this course.
    let label_id = {
        ctx.current_board
            .labels
            .iter()
            .find(|l| l.name == mapping.trello_label_name)
            .wrap_err_with(|| format!("Could not find label {:?}", mapping.trello_label_name))?
            .id
            .clone()
    };

    // Format the description for this card.
    let desc_header = "🔄 Canvas Trello Sync";
    let new_description = match &assignment.description {
        Some(description) => {
            let desc_md = html2md::parse_html(&description);
            format!("{desc_header}\n\n---\n\n{desc_md}")
        }
        None => desc_header.to_string(),
    };

    // Check if the card with that Canvas URL already exists.
    let cards_with_correct_url = ctx
        .current_board
        .cards
        .iter()
        .filter(|c| {
            c.custom_field_items
                .iter()
                .find(|i| i.id_custom_field == ctx.canvas_url_field_id)
                .and_then(|f| f.as_str())
                == Some(canvas_url.as_str())
        })
        .collect::<Vec<_>>();

    // Update any existing cards with the right due date.
    for existing_card in &cards_with_correct_url {
        let mismatch_due = existing_card.due != Some(assignment.due_at);
        let mismatch_complete = existing_card.due_complete != assignment.submitted();
        let mismatch_desc =
            existing_card.desc.starts_with(desc_header) && existing_card.desc != new_description;

        let should_update = mismatch_due || mismatch_complete || mismatch_desc;

        if !should_update {
            // The card already exists and has the right due date.
            info!(card_id=%existing_card.id, due=%assignment.due_at, complete=%assignment.submitted(), "Card up to date");
            ctx.count_up_to_date.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        debug!(
            mismatch_due,
            mismatch_complete, mismatch_desc, "Card needs update"
        );
        debug!(
            old_desc = existing_card.desc,
            new_desc = &new_description,
            "Description"
        );

        // Update the card.
        info!(card_id=%existing_card.id, due=%assignment.due_at, complete=%assignment.submitted(), "Update card");
        let patch = [
            ("due", assignment.due_at.to_rfc3339()),
            ("dueComplete", assignment.submitted().to_string()),
            ("desc", new_description.to_owned()),
        ];
        ctx.trello
            .update_card(&existing_card.id, patch)
            .await
            .wrap_err("Failed to update card")?;
        ctx.count_updated.fetch_add(1, Ordering::Relaxed);
    }

    // If there are no existing cards, create one.
    if cards_with_correct_url.is_empty() {
        info!(assignment_name=%assignment.name, due=%assignment.due_at, "Create card");

        // Create the new card.
        let new_card_fields = lib::trello::CreateCard {
            name: assignment.name.clone(),
            desc: new_description,
            due: assignment.due_at,
            due_complete: assignment.submitted(),
            label_ids: vec![label_id.clone()],
        };
        let new_card = ctx
            .trello
            .create_card(&ctx.new_card_list_id, new_card_fields)
            .await
            .wrap_err("Failed to create card")?;

        // Set the Canvas URL custom field.
        let custom_field_value = lib::trello::CustomFieldValue::Text {
            text: canvas_url.to_string(),
        };
        ctx.trello
            .set_card_custom_field(&new_card.id, &ctx.canvas_url_field_id, custom_field_value)
            .await
            .wrap_err("Failed to set Canvas URL custom field")?;

        ctx.count_created.fetch_add(1, Ordering::Relaxed);
    }

    Ok(())
}
