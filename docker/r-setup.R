dir.create("/opt/r-library", recursive = TRUE, showWarnings = FALSE)
renv::restore(
    library  = "/opt/r-library",
    lockfile = file.path(getwd(), "renv.lock"),
    confirm  = FALSE,
    clean    = FALSE
)
if (file.exists("DESCRIPTION")) {
    desc <- read.dcf("DESCRIPTION")
    if ("Suggests" %in% colnames(desc)) {
        suggests <- trimws(strsplit(gsub("\\s*\\([^)]*\\)", "", desc[,"Suggests"]), ",")[[1]])
        suggests <- suggests[nzchar(suggests)]
        if (length(suggests)) {
            install.packages(suggests, lib = "/opt/r-library", repos = "https://cloud.r-project.org")
        }
    }
}
