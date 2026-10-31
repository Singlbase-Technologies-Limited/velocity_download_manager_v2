# AI agent account

Claude Code works on this repository as a dedicated GitHub **machine account**, so its pull requests can be reviewed and approved by the Product Owner. GitHub does not let authors approve their own pull requests. This page tells any agent or chat session which identity to use, and how to check it before writing anything to GitHub.

Decision recorded in #89 (2026-10-21). Since 2026-10-31 the repository is public and owned by Singlbase Technologies Limited, and the agent works on it as an outside collaborator (#234).

## Which account for what

| Work | Account | How |
|---|---|---|
| This repository (`Singlbase-Technologies-Limited/velocity_download_manager_v2`): commits, branches, pull requests, issues, comments, project board | **`velocity-claude-agent`** | git remote alias `github-velocity-agent`; `gh` with the agent's token (below) |
| Approving, releases, repository settings | **`vincentarisawork`** (Product Owner) | Done by the Product Owner, never by the agent |
| Merging | **`vincentarisawork`**, or **`velocity-claude-agent`** for one PR the PO explicitly approved ("I approve the PR") | Squash merge; see [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md#merging) |
| Other organisation repositories (`Singlbase-Technologies-Limited/*`, including V1) | **`vincentarisawork`** | The agent is not an organisation member, which would use a paid seat (PO decision). It is an outside collaborator on this repository only, which costs no seat while the repository is public |

- **What the agent can do here:** `velocity-claude-agent` is an outside collaborator with **Write** access to this repository and to the organisation's project 4 (*Velocity Download Manager V2*, [the board](https://github.com/orgs/Singlbase-Technologies-Limited/projects/4)).
- **What it can't do:** it has no admin rights, so it can't change settings or merge outside the normal PR flow. The rulesets also stop it pushing to `main` or creating a release tag (`v*`): only the PO does that ([REPOSITORY_SETTINGS.md](REPOSITORY_SETTINGS.md)).
- **Terms:** it is a GitHub machine account operated by `vincentarisawork`, who is responsible for its actions ([GitHub Terms of Service](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service)).

## How it is wired on the Product Owner's PC

The PO's own account stays the default everywhere. Only this repository is pointed at the agent.

| Piece | Setting |
|---|---|
| SSH key | `~/.ssh/id_ed25519_velocity_agent`, registered on the agent account only. Never commit or print the private key |
| SSH alias (`~/.ssh/config`) | `Host github-velocity-agent` → `github.com`, `IdentityFile ~/.ssh/id_ed25519_velocity_agent`, `IdentitiesOnly yes` |
| Git remote (this repository) | `git@github-velocity-agent:Singlbase-Technologies-Limited/velocity_download_manager_v2.git` |
| Git identity (repository-local) | `user.name = velocity-claude-agent`, `user.email = 345876015+velocity-claude-agent@users.noreply.github.com` |
| GitHub CLI | Both accounts are logged in (`gh auth status`); **`vincentarisawork` is active**. Agent commands pass the agent's token explicitly |

## Using it (agents)

Run every `gh` command for this repository with the agent's token.

**Bash (Git Bash, WSL):**

```sh
GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh pr create --base main …
```

**PowerShell:**

```powershell
$env:GH_TOKEN = gh auth token -u velocity-claude-agent
gh pr create --base main …
Remove-Item Env:GH_TOKEN
```

**Rules:**

- Never run `gh auth switch` to make the agent the default. Other work on this PC, such as the organisation repositories, relies on the PO's account being active.
- Never use the agent account for organisation repositories, and never use the PO's account to author pull requests here. The PO couldn't approve them.
- Never print, log or commit tokens or private keys. `gh auth token` output goes only into the `GH_TOKEN` variable of the command that needs it.

### Check before you push or open a PR

```sh
git remote get-url origin        # must be git@github-velocity-agent:Singlbase-Technologies-Limited/velocity_download_manager_v2.git
git config user.email            # must be 345876015+velocity-claude-agent@users.noreply.github.com
ssh -T git@github-velocity-agent # "Hi velocity-claude-agent! …"
GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh api user --jq .login   # velocity-claude-agent
```

If any of these shows a different account, or the agent's login is missing (`gh auth token -u velocity-claude-agent` fails), **stop and ask the Product Owner**. Never fall back to the PO's account.

## Setting it up on another machine (Product Owner)

1. **Clone through the alias.** Create a key with `ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_velocity_agent` and add the `github-velocity-agent` alias to `~/.ssh/config` (see the table above). Clone with `git clone git@github-velocity-agent:Singlbase-Technologies-Limited/velocity_download_manager_v2.git`.
2. **Set the repository-local identity:**
   ```sh
   git config user.name velocity-claude-agent
   git config user.email 345876015+velocity-claude-agent@users.noreply.github.com
   ```
3. **Log the agent into `gh`.** Open a **private browser window** logged in as `velocity-claude-agent`, then run:
   ```sh
   gh auth login --hostname github.com --git-protocol ssh --web --skip-ssh-key --scopes "project,admin:public_key"
   ```
   Enter the one-time code at `https://github.com/login/device` **in the private window**. The tab that opens automatically is logged in as the PO.
4. **Register the key and restore the default account:**
   ```sh
   GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh ssh-key add ~/.ssh/id_ed25519_velocity_agent.pub --title "<machine name>"
   gh auth switch --user vincentarisawork
   ```
5. **Verify** with the checks above.
