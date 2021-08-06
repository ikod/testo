use clap::{App, Arg, ArgMatches};

pub fn parse_args() -> ArgMatches {
    let args = App::new("testo")
        .arg(
            Arg::new("task_pool")
                .default_value("64")
                .required(false)
                .short('n'),
        )
        .arg(
            Arg::new("loglvl")
                .default_value("info")
                .required(false)
                .short('l'),
        )
        .get_matches();
    args
}
