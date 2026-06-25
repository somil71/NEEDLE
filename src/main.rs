use clap::Parser;
use needle::Result;

mod cli;

#[derive(Parser)]
#[command(name = "needle")]
#[command(version = "0.1.0")]
#[command(about = "Local-first hybrid search engine — keyword + semantic, offline, instant")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Initialize and build the search index for one or more directories
    Init {
        /// Directories to index
        #[arg(required = true)]
        directories: Vec<String>,
    },

    /// Search across indexed content (keyword + semantic hybrid)
    Search {
        /// Search query (exact term, identifier, or natural language description)
        query: String,

        /// Maximum number of results to return
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Show all results above threshold
        #[arg(long)]
        all: bool,

        /// Disable colored output
        #[arg(long)]
        no_color: bool,

        /// Compact output (one line per result, no snippet)
        #[arg(long)]
        compact: bool,

        /// Filter results to a specific language (rs, py, ts, js, go, md, …)
        #[arg(short, long)]
        lang: Option<String>,
    },

    /// Show index health and statistics
    Status,

    /// Force full re-index of all watched directories
    Reindex,

    /// View or edit configuration
    Config {
        #[command(subcommand)]
        action: Option<cli::config::ConfigAction>,
    },

    /// Run performance benchmarks (HNSW recall, query latency, index size)
    Bench,

    /// Watch indexed directories and re-index on file changes
    Watch,

    /// Start an MCP server over stdio for AI agent integration
    ///
    /// Add to agent config:
    ///   { "mcpServers": { "needle": { "command": "needle", "args": ["mcp"] } } }
    Mcp,

    /// Launch the web search UI in your browser
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "7700")]
        port: u16,

        /// Don't open the browser automatically
        #[arg(long)]
        no_open: bool,
    },

    /// Generate a Markdown report: god nodes, communities, and surprise edges
    Report {
        /// Output file path (default: GRAPH_REPORT.md)
        #[arg(short, long)]
        output: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "needle=warn".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { directories } => cli::init::run(directories).await?,
        Commands::Search { query, limit, all, no_color, compact, lang } => {
            cli::search::run(query, limit, all, no_color, compact, lang).await?
        }
        Commands::Status => cli::status::run().await?,
        Commands::Reindex => cli::reindex::run().await?,
        Commands::Config { action } => cli::config::run(action).await?,
        Commands::Bench => cli::bench::run().await?,
        Commands::Watch => cli::watch::run().await?,
        Commands::Mcp => cli::mcp::run().await?,
        Commands::Serve { port, no_open } => cli::serve::run(port, no_open).await?,
        Commands::Report { output } => cli::report::run(output).await?,
    }

    Ok(())
}
