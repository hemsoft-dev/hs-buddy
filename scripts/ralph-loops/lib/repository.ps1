# lib/repository.ps1 — Shared GitHub origin and worktree checks for ralph-loops scripts.
# Version: 1.0.0
# Dot-source from any ralph script:  . "$PSScriptRoot/lib/repository.ps1"
#
# Exports:
#   ConvertFrom-RalphRemoteUrl -Url <url>          → @{ Host; Slug; Ssh } for an HTTP(S), SSH or scp-like URL, else $null
#   Test-RalphGitHubHost       -HostName <host>    → $true for github.com and its subdomains
#   Resolve-RalphSshHost       -HostName <alias>   → the HostName `ssh -G` reports, or $null
#   Get-RalphOriginRemote      [-ResolveSshHost]   → @{ Slug } when origin names a GitHub repository, else @{ Error }
#   Confirm-RalphGitHubRepo    -Slug <owner/repo>  → the repository's name as gh resolves it, or $null
#   Test-RalphReusedWorktree   -Path -Branch       → $null when the worktree is ready, else why it is not
#   Invoke-RalphRebaseOnto     -Base <branch>      → $null when the branch was rebased onto origin's Base, else why not

# Reads a remote URL the way Buddy Native's crew::project::parse_remote does. A local path
# or file:// URL is no remote of any host, so it never yields a slug.
function ConvertFrom-RalphRemoteUrl {
    param([string]$Url)
    $Url = "$Url".Trim()
    $ssh = $false
    if ($Url -match '^(https?|ssh)://([^/]*)/(.*)$') {
        $scheme = $Matches[1].ToLowerInvariant()
        $hostName = ($Matches[2] -split '@')[-1]
        $path = $Matches[3]
        if ($scheme -eq 'ssh') {
            $hostName = ($hostName -split ':')[0]
            $ssh = $true
        }
    }
    elseif ($Url -match '^([^/:]+):(.+)$' -and -not $Matches[2].StartsWith('/')) {
        # scp-like: [user@]host:owner/repo
        $hostName = ($Matches[1] -split '@')[-1]
        $path = $Matches[2]
        $ssh = $true
    }
    else {
        return $null
    }
    $path = $path.TrimEnd('/')
    if ($path.EndsWith('.git')) { $path = $path.Substring(0, $path.Length - 4) }
    if (-not $hostName -or $path -cnotmatch '^([A-Za-z0-9_.-]{1,100})/([A-Za-z0-9_.-]{1,100})$') { return $null }
    if ($Matches[1] -in @('.', '..') -or $Matches[2] -in @('.', '..')) { return $null }
    return [pscustomobject]@{ Host = $hostName.ToLowerInvariant(); Slug = $path; Ssh = $ssh }
}

function Test-RalphGitHubHost {
    param([string]$HostName)
    $name = "$HostName".Trim().ToLowerInvariant()
    return $name -eq 'github.com' -or $name.EndsWith('.github.com')
}

# The real host behind an SSH alias such as `github-work`, as gh resolves it.
function Resolve-RalphSshHost {
    param([string]$HostName)
    if (-not $HostName -or $HostName.StartsWith('-')) { return $null }
    $ssh = Get-Command ssh -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $ssh) { return $null }
    $config = & $ssh.Source -G $HostName 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    foreach ($line in $config) {
        if ($line -match '^hostname\s+(\S+)') { return $Matches[1] }
    }
    return $null
}

# The GitHub repository origin names, read from its URL alone. Anything else (no origin, a
# local path, another host) is refused, so a loop never acts on a same-named GitHub
# repository. The URL is not echoed: it can hold a token.
function Get-RalphOriginRemote {
    param([scriptblock]$ResolveSshHost = { param($HostName) Resolve-RalphSshHost -HostName $HostName })
    $url = git --no-pager remote get-url origin 2>$null
    if ($LASTEXITCODE -ne 0 -or -not "$url".Trim()) {
        return @{ Error = "This repository has no origin remote. Ralph works on the GitHub repository origin names." }
    }
    $remote = ConvertFrom-RalphRemoteUrl -Url "$url"
    $github = $remote -and ((Test-RalphGitHubHost -HostName $remote.Host) -or
        ($remote.Ssh -and (Test-RalphGitHubHost -HostName (& $ResolveSshHost $remote.Host))))
    if (-not $github) {
        return @{ Error = "origin is not a GitHub repository (git remote get-url origin). Ralph only works on GitHub repositories." }
    }
    return @{ Slug = $remote.Slug }
}

# Asks gh for the repository on github.com; it also confirms the active (or matched)
# account can read it. Returns the name gh reports (it follows renames), or $null.
function Confirm-RalphGitHubRepo {
    param([Parameter(Mandatory)][string]$Slug)
    $name = gh repo view "github.com/$Slug" --json nameWithOwner -q '.nameWithOwner' 2>$null
    if ($LASTEXITCODE -ne 0 -or -not "$name".Trim()) { return $null }
    return "$name".Trim()
}

# A worktree left by an earlier run is used only when it is clean, tracks origin's Branch
# and fast-forwards to it, so the agent never works on a stale or diverged head.
function Test-RalphReusedWorktree {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Branch
    )
    $status = git -C $Path status --porcelain 2>$null
    if ($LASTEXITCODE -ne 0) {
        return "git status failed there."
    }
    if ($status) {
        return "it has uncommitted changes. Commit, stash or discard them first."
    }
    $upstream = git -C $Path rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>$null
    if ($LASTEXITCODE -ne 0 -or -not "$upstream".Trim()) {
        return "branch '$Branch' has no upstream. Set it with: git -C '$Path' branch --set-upstream-to origin/$Branch"
    }
    if ("$upstream".Trim() -ne "origin/$Branch") {
        return "branch '$Branch' tracks $("$upstream".Trim()), not origin/$Branch."
    }
    $pull = git -C $Path pull --ff-only 2>&1
    if ($LASTEXITCODE -ne 0) {
        return "it could not fast-forward to origin/$Branch (git pull --ff-only): $(($pull | Out-String).Trim())"
    }
    return $null
}

# Fetches Base from origin into origin/Base (whatever the clone's fetch refspec) and
# rebases the checked-out branch onto it without hooks. A failed rebase is aborted.
function Invoke-RalphRebaseOnto {
    param([Parameter(Mandatory)][string]$Base)
    $fetch = git fetch origin "+refs/heads/${Base}:refs/remotes/origin/${Base}" 2>&1
    if ($LASTEXITCODE -ne 0) {
        return "git fetch origin $Base failed: $(($fetch | Out-String).Trim())"
    }
    $rebase = git -c core.hooksPath=NUL rebase "origin/$Base" 2>&1
    if ($LASTEXITCODE -ne 0) {
        git rebase --abort 2>&1 | Out-Null
        return "git rebase origin/$Base failed: $(($rebase | Out-String).Trim())"
    }
    return $null
}
