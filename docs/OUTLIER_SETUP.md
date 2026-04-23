# Adding Outlier Playground to llm-conductor

Outlier Playground provides free access to 20+ frontier AI models through your RLHF contract work, including:
- **Claude Opus 4.6** (the main attraction!)
- Claude Sonnet 4.6
- GPT-5.2, GPT-5.1, o3
- Gemini 3.1 Pro
- Grok 3
- DeepSeek v3.2

## Quick Setup (Manual Method - Recommended)

The easiest and most reliable way:

### Step 1: Get Cookies from Browser

1. Open Vivaldi and go to https://playground.outlier.ai (make sure you're logged in)
2. Press **F12** to open Developer Tools
3. Go to **Application** tab → **Cookies** → `https://playground.outlier.ai`
4. You need two things:
   - **Cookie string**: Right-click any cookie → **Copy** → **Copy all as cURL** (look for the Cookie: header)
   - **CSRF token**: Find the `_csrf` cookie and copy its Value

### Step 2: Add to llm-conductor

```bash
cd ~/personal/llm-conductor

# Add the cookie string (all cookies in format: name1=value1; name2=value2; ...)
cargo run --release -- config add-key outlier_cookie 'PASTE_YOUR_COOKIE_STRING'

# Add the CSRF token (just the value of _csrf cookie)
cargo run --release -- config add-key outlier_csrf 'PASTE_YOUR_CSRF_TOKEN'
```

### Step 3: Test

```bash
# Rebuild NixOS to get updated version
cd ~/JCH-NixOS
nix flake update llm-conductor
sudo nixos-rebuild switch --flake .#jch-wsl

# Check providers
llm-conductor providers

# Start chatting with Opus!
llm-conductor
```

## Cookie Format Example

**Cookie string** (all one line):
```
_session=eyJhbG...; _csrf=abc123xyz; _jwt=eyJhbG...; _t=MTY5OT...; analytics_session_id=...
```

**CSRF token** (standalone value):
```
abc123xyz
```

## Troubleshooting

**❌ No cookies found / Empty values:**
- Use the manual Dev Tools method above
- Windows encrypts cookies - automated extraction is complex

**❌ Authentication errors (401/403):**
- Cookies expire - re-extract them
- Make sure you copied the complete string

**❌ "No conversations available":**
- Go to https://playground.outlier.ai
- Start at least one conversation first
- llm-conductor reuses that conversation

## Automated Scripts (Advanced)

Two scripts are provided in `scripts/` but may require setup:

1. **Extract-OutlierCookies.ps1** - Run in Windows PowerShell
2. **extract-outlier-windows.py** - Run in WSL (requires PowerShell access)

Due to Windows DPAPI encryption, **manual extraction is currently more reliable**.

## Security

- Cookies are stored in `~/.config/llm-conductor/credentials.json` (chmod 600)
- Never share your cookies - they're credentials to your account
- They expire and can be refreshed by re-extracting
