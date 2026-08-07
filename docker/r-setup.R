lib <- Sys.getenv("R_LIBS_SITE", "/opt/r-library")
repos <- Sys.getenv("R_REPOSITORIES", "https://cloud.r-project.org")
dir.create(lib, recursive = TRUE, showWarnings = FALSE)
renv::restore(
    library  = lib,
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
            install.packages(suggests, lib = lib, repos = repos)
        }
    }
}
