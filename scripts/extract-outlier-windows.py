#!/usr/bin/env python3
"""Extract Outlier cookies from Windows browsers in WSL"""

import sqlite3, shutil, tempfile, sys, subprocess
from pathlib import Path

def get_path(browser, profile):
    base = Path("/mnt/c/Users/joshu/AppData/Local")
    paths = {
        "vivaldi": base / "Vivaldi" / "User Data" / profile / "Network" / "Cookies",
        "chrome": base / "Google/Chrome" / "User Data" / profile / "Network" / "Cookies",
        "edge": base / "Microsoft/Edge" / "User Data" / profile / "Network" / "Cookies",
    }
    return paths.get(browser.lower())

def decrypt(encrypted_hex):
    ps = f"""
    $b = [byte[]]@({encrypted_hex})
    $d = [System.Security.Cryptography.ProtectedData]::Unprotect($b, $null, 'CurrentUser')
    [Text.Encoding]::UTF8.GetString($d)
    """
    r = subprocess.run(["powershell.exe", "-Command", ps], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else None

browser = sys.argv[1] if len(sys.argv) > 1 else "vivaldi"
profile = sys.argv[2] if len(sys.argv) > 2 else "Default"

path = get_path(browser, profile)
if not path or not path.exists():
    print(f"❌ {browser.title()} cookies not found. Log in to playground.outlier.ai first.")
    sys.exit(1)

temp = Path(tempfile.gettempdir()) / "outlier_temp.db"
shutil.copy(path, temp)

conn = sqlite3.connect(str(temp))
rows = conn.execute("SELECT name, encrypted_value FROM cookies WHERE host_key LIKE '%outlier.ai%'").fetchall()
conn.close()
temp.unlink()

if not rows:
    print("❌ No Outlier cookies found")
    sys.exit(1)

print(f"🔍 Decrypting {len(rows)} cookies...")
cookies = {}
for name, enc in rows:
    if enc:
        hex_str = ",".join(f"0x{b:02x}" for b in enc)
        val = decrypt(hex_str)
        if val:
            cookies[name] = val
            print(f"   ✓ {name}")

cookie_str = "; ".join(f"{k}={v}" for k, v in cookies.items())
csrf = cookies.get("_csrf", "")

print("\n" + "="*70)
print("\n✅ Extracted! Run these commands:\n")
print(f"cd ~/personal/llm-conductor")
print(f"cargo run --release -- config add-key outlier_cookie '{cookie_str}'")
if csrf:
    print(f"cargo run --release -- config add-key outlier_csrf '{csrf}'")
