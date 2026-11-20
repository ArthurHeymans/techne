;;; Forge: GitHub issue, pull-request, notification and code-review management
;;; for Techne, inspired by Emacs Magit Forge and code-review. Works with both
;;; Git and Jujutsu repositories in `vc-status`, with a local SQLite cache.
(package forge
  (editor "editor.scm"
    (commands
      (forge-dispatch "Open the Forge command menu.")
      (forge-pull "Pull issues and pull requests for the current repository from GitHub.")
      (forge-pull-topic "Pull a single issue or pull request by number.")
      (forge-pull-notifications "Pull GitHub notifications into the local database.")
      (forge-add-repository "Track a GitHub repository in the local Forge database.")
      (forge-list-topics "List cached issues and pull requests for the current repository.")
      (forge-list-pullreqs "List cached pull requests for the current repository.")
      (forge-list-issues "List cached issues for the current repository.")
      (forge-list-global-topics "List cached topics across all tracked repositories.")
      (forge-list-notifications "List cached GitHub notifications.")
      (forge-list-repositories "List repositories tracked in the local Forge database.")
      (forge-visit-topic "Choose and inspect a cached topic.")
      (forge-visit-topic-from-url "Inspect a GitHub issue or pull request by URL.")
      (forge-create-issue "Compose and submit a new GitHub issue.")
      (forge-create-pullreq "Compose and submit a new GitHub pull request.")
      (forge-checkout-pullreq "Fetch and check out a pull request in Git or Jujutsu.")
      (forge-browse "Open the Forge topic, repository, branch, commit or file in a browser.")
      (forge-copy-url "Copy the GitHub URL of the topic, commit or file at point.")
      (code-review-start "Open a pull request in the interactive code-review buffer.")
      (code-review-forge-pr-at-point "Start a code review for the pull request at point."))
    (options
      (forge-database #f any
        "Local Forge SQLite file, or #f for the XDG data directory.")
      (forge-credential-provider #f any
        "Procedure (host user) returning a GitHub token, or #f.
May return a string, `secret` or `secret-ref`, overriding
`secret-sources` when set. Secrets are never persisted or logged.")
      (forge-alist (("github.com" "api.github.com" "github.com" github)) any
        "Known forge hosts as (GIT-HOST API-HOST WEB-HOST KIND) entries.
Add entries for GitHub Enterprise instances.")
      (forge-status-sections #t boolean
        "Whether tracked repos show Pull requests and Issues in `vc-status`.")
      (forge-status-topic-limit 10 any
        "Maximum open topics shown per Forge section in `vc-status`.")
      (forge-allow-gh-cli-token #t boolean
        "Whether to fall back to `gh auth token` when no token is configured.")
      (forge-draft-by-default #f boolean
        "Whether newly created pull requests start marked as drafts."))))
