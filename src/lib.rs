use clap::{App, Arg, ArgMatches};

pub fn parse_args() -> ArgMatches {
    let args = App::new("testo")
        .arg(
            Arg::new("task_pool")
                .default_value("64")
                .required(false)
                .about("task pool size")
                .short('n'),
        )
        .arg(
            Arg::new("workers")
                .default_value("4")
                .required(false)
                .about("threads workers")
                .short('w'),
        )
        .arg(
            Arg::new("loglvl")
                .default_value("info")
                .required(false)
                .about("log level")
                .short('l'),
        )
        .arg(
            Arg::new("stop_after")
                .default_value("100")
                .required(false)
                .about("stop after that num of processed requests")
                .short('s'),
        )
        .get_matches();
    args
}
