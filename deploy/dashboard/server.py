#!/usr/bin/env python3
"""
Micro-PaaS Web Dashboard
A lightweight (12MB RAM), zero-Docker app and secret management dashboard for Ubuntu VPS.
Manages both Zero-Downtime Blue/Green systemd apps and legacy Piku apps.
"""

import os
import sys
import json
import subprocess
import shutil
import re
import urllib.request
import urllib.parse
from http.server import HTTPServer, BaseHTTPRequestHandler
import base64
import hashlib
import secrets

CONFIG_DIR = "/opt/micro-dashboard"
CREDS_FILE = os.path.join(CONFIG_DIR, ".credentials")
PORT = 8888
HOST = "127.0.0.1"

# Default admin credentials if not set
DEFAULT_USER = "admin"
DEFAULT_PASS = "admin12345"

def get_auth_credentials():
    if os.path.exists(CREDS_FILE):
        try:
            with open(CREDS_FILE, "r") as f:
                line = f.read().strip()
                if ":" in line:
                    u, p = line.split(":", 1)
                    return u.strip(), p.strip()
        except Exception:
            pass
    return DEFAULT_USER, DEFAULT_PASS

def set_auth_credentials(username, password):
    os.makedirs(CONFIG_DIR, exist_ok=True)
    with open(CREDS_FILE, "w") as f:
        f.write(f"{username}:{password}\n")
    os.chmod(CREDS_FILE, 0o600)

def check_auth(headers):
    auth_header = headers.get("Authorization")
    if not auth_header or not auth_header.startswith("Basic "):
        return False
    try:
        decoded = base64.b64decode(auth_header[6:]).decode("utf-8")
        user, pwd = decoded.split(":", 1)
        expected_user, expected_pwd = get_auth_credentials()
        return secrets.compare_digest(user, expected_user) and secrets.compare_digest(pwd, expected_pwd)
    except Exception:
        return False

def run_cmd(cmd, timeout=10):
    try:
        res = subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=timeout)
        return res.returncode, res.stdout.strip(), res.stderr.strip()
    except Exception as e:
        return 1, "", str(e)

def get_system_stats():
    # RAM
    total_mem, free_mem, avail_mem = 0, 0, 0
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal:"):
                    total_mem = int(line.split()[1]) // 1024
                elif line.startswith("MemAvailable:"):
                    avail_mem = int(line.split()[1]) // 1024
        used_mem = total_mem - avail_mem
    except Exception:
        total_mem, used_mem, avail_mem = 1024, 0, 1024

    # CPU load
    try:
        with open("/proc/loadavg") as f:
            load = f.read().split()[:3]
    except Exception:
        load = ["0.00", "0.00", "0.00"]

    # Disk
    try:
        du = shutil.disk_usage("/")
        disk_total = du.total // (1024 * 1024 * 1024)
        disk_used = du.used // (1024 * 1024 * 1024)
        disk_pct = int((du.used / du.total) * 100)
    except Exception:
        disk_total, disk_used, disk_pct = 10, 2, 20

    # Uptime
    try:
        with open("/proc/uptime") as f:
            up_secs = int(float(f.read().split()[0]))
            days = up_secs // 86400
            hours = (up_secs % 86400) // 3600
            uptime_str = f"{days}d {hours}h"
    except Exception:
        uptime_str = "unknown"

    # Engines detection
    has_blue_green = os.path.exists("/opt/apps") and os.path.exists("/etc/nginx/upstreams")
    has_piku = os.path.exists("/home/piku/.piku")
    piku_active = False
    if has_piku:
        _, out, _ = run_cmd("systemctl is-active uwsgi-piku")
        piku_active = (out == "active")

    # Firewall status
    _, ufw_out, _ = run_cmd("ufw status")
    ufw_active = "Status: active" in ufw_out

    return {
        "ram": {"total_mb": total_mem, "used_mb": used_mem, "avail_mb": avail_mem, "pct": int((used_mem / total_mem) * 100) if total_mem else 0},
        "cpu_load": load,
        "disk": {"total_gb": disk_total, "used_gb": disk_used, "pct": disk_pct},
        "uptime": uptime_str,
        "engines": {
            "blue_green_engine": has_blue_green,
            "piku_engine_installed": has_piku,
            "piku_engine_active": piku_active
        },
        "firewall": {"active": ufw_active, "raw": ufw_out}
    }

def get_blue_green_apps():
    apps = []
    if not os.path.exists("/opt/apps"):
        return apps

    for entry in os.listdir("/opt/apps"):
        app_path = os.path.join("/opt/apps", entry)
        if not os.path.isdir(app_path):
            continue

        app_name = entry
        upstream_link = f"/etc/nginx/upstreams/{app_name}.conf"
        active_slot = "unknown"
        if os.path.islink(upstream_link):
            target = os.readlink(upstream_link)
            if "green" in target:
                active_slot = "green"
            elif "blue" in target:
                active_slot = "blue"

        # Determine ports
        blue_port, green_port = "unknown", "unknown"
        blue_conf = f"/etc/nginx/upstreams/{app_name}-blue.conf"
        green_conf = f"/etc/nginx/upstreams/{app_name}-green.conf"
        if os.path.exists(blue_conf):
            try:
                with open(blue_conf) as f:
                    m = re.search(r":(\d+)", f.read())
                    if m: blue_port = m.group(1)
            except Exception: pass
        if os.path.exists(green_conf):
            try:
                with open(green_conf) as f:
                    m = re.search(r":(\d+)", f.read())
                    if m: green_port = m.group(1)
            except Exception: pass

        # Service statuses
        _, blue_status, _ = run_cmd(f"systemctl is-active {app_name}-blue")
        _, green_status, _ = run_cmd(f"systemctl is-active {app_name}-green")

        # Active port & health check
        current_port = green_port if active_slot == "green" else blue_port
        healthy = False
        if current_port and current_port != "unknown":
            try:
                req = urllib.request.Request(f"http://127.0.0.1:{current_port}/healthz", method="GET")
                with urllib.request.urlopen(req, timeout=1) as resp:
                    healthy = (resp.status == 200)
            except Exception:
                healthy = False

        env_exists = os.path.exists(os.path.join(app_path, "shared", ".env"))

        apps.append({
            "name": app_name,
            "engine": "blue_green",
            "active_slot": active_slot,
            "blue": {"port": blue_port, "status": blue_status},
            "green": {"port": green_port, "status": green_status},
            "current_port": current_port,
            "healthy": healthy,
            "env_configured": env_exists,
            "path": app_path
        })
    return apps

def get_piku_apps():
    apps = []
    piku_apps_dir = "/home/piku/.piku/apps"
    if not os.path.exists(piku_apps_dir):
        return apps

    for entry in os.listdir(piku_apps_dir):
        app_path = os.path.join(piku_apps_dir, entry)
        if not os.path.isdir(app_path):
            continue

        app_name = entry
        # Check Piku config
        _, out, _ = run_cmd(f"sudo -iu piku python3 /home/piku/piku.py ps {app_name} 2>/dev/null")
        status = "running" if "web:1" in out or "1" in out else "stopped"

        # Check port from nginx
        port = "unknown"
        nginx_conf = f"/home/piku/.piku/nginx/{app_name}.conf"
        if os.path.exists(nginx_conf):
            try:
                with open(nginx_conf) as f:
                    m = re.search(r"127\.0\.0\.1:(\d+)", f.read())
                    if m: port = m.group(1)
            except Exception: pass

        env_exists = os.path.exists(os.path.join(app_path, "ENV"))

        apps.append({
            "name": app_name,
            "engine": "piku",
            "active_slot": "piku-worker",
            "current_port": port,
            "status": status,
            "healthy": (status == "running"),
            "env_configured": env_exists,
            "path": app_path
        })
    return apps

def read_env_file(app_name, engine):
    vars_list = []
    if engine == "blue_green":
        env_path = f"/opt/apps/{app_name}/shared/.env"
    else:
        env_path = f"/home/piku/.piku/apps/{app_name}/ENV"

    if os.path.exists(env_path):
        try:
            with open(env_path, "r") as f:
                for line in f:
                    line = line.strip()
                    if not line or line.startswith("#"):
                        continue
                    if "=" in line:
                        k, v = line.split("=", 1)
                        vars_list.append({"key": k.strip(), "value": v.strip()})
        except Exception as e:
            print("Error reading env:", e)
    return vars_list

def save_env_file(app_name, engine, vars_list):
    if engine == "blue_green":
        env_dir = f"/opt/apps/{app_name}/shared"
        os.makedirs(env_dir, exist_ok=True)
        env_path = os.path.join(env_dir, ".env")
        with open(env_path, "w") as f:
            for item in vars_list:
                k = item.get("key", "").strip()
                v = item.get("value", "").strip()
                if k:
                    f.write(f"{k}={v}\n")
        os.chmod(env_path, 0o600)
        # Sudo restart active slot
        upstream_link = f"/etc/nginx/upstreams/{app_name}.conf"
        slot = "green"
        if os.path.islink(upstream_link) and "blue" in os.readlink(upstream_link):
            slot = "blue"
        run_cmd(f"systemctl restart {app_name}-{slot}")
    else:
        # Piku app
        for item in vars_list:
            k = item.get("key", "").strip()
            v = item.get("value", "").strip()
            if k:
                run_cmd(f"sudo -iu piku python3 /home/piku/piku.py config:set {app_name} {k}={v}")
    return True

def get_app_logs(app_name, engine, lines=100):
    if engine == "blue_green":
        # Check active slot
        upstream_link = f"/etc/nginx/upstreams/{app_name}.conf"
        slot = "green"
        if os.path.islink(upstream_link) and "blue" in os.readlink(upstream_link):
            slot = "blue"
        _, out, err = run_cmd(f"journalctl -u {app_name}-{slot} -n {lines} --no-pager")
        return out or err
    else:
        log_file = f"/home/piku/.piku/logs/{app_name}/web.1.log"
        if os.path.exists(log_file):
            _, out, _ = run_cmd(f"tail -n {lines} {log_file}")
            return out
        return "No log file found."

def create_new_app(data):
    app_name = data.get("app_name", "").lower().strip()
    domain = data.get("domain", "").strip()
    stack = data.get("stack", "rust")
    engine = data.get("engine", "blue_green")

    if not app_name or not re.match(r"^[a-z0-9-]+$", app_name):
        return False, "Invalid app name. Use lowercase letters, numbers, and dashes."

    if engine == "blue_green":
        # Find next available port pair starting at 4001
        used_ports = set()
        if os.path.exists("/etc/nginx/upstreams"):
            for f in os.listdir("/etc/nginx/upstreams"):
                try:
                    with open(os.path.join("/etc/nginx/upstreams", f)) as uf:
                        for m in re.finditer(r":(\d+)", uf.read()):
                            used_ports.add(int(m.group(1)))
                except Exception: pass

        port_blue = 4001
        while port_blue in used_ports or (port_blue + 1) in used_ports:
            port_blue += 2
        port_green = port_blue + 1

        app_root = f"/opt/apps/{app_name}"
        os.makedirs(f"{app_root}/bin", exist_ok=True)
        os.makedirs(f"{app_root}/shared", exist_ok=True)

        # Copy switch script
        if os.path.exists("/opt/apps/todo/bin/switch.sh"):
            shutil.copy2("/opt/apps/todo/bin/switch.sh", f"{app_root}/bin/switch.sh")
            os.chmod(f"{app_root}/bin/switch.sh", 0o755)

        run_cmd(f"chown -R deploy:www-data {app_root} && chmod -R 775 {app_root}")

        # Create upstream configs
        os.makedirs("/etc/nginx/upstreams", exist_ok=True)
        with open(f"/etc/nginx/upstreams/{app_name}-blue.conf", "w") as f:
            f.write(f"upstream {app_name}_backend {{ server 127.0.0.1:{port_blue}; }}\n")
        with open(f"/etc/nginx/upstreams/{app_name}-green.conf", "w") as f:
            f.write(f"upstream {app_name}_backend {{ server 127.0.0.1:{port_green}; }}\n")

        # Symlink default to blue
        target_link = f"/etc/nginx/upstreams/{app_name}.conf"
        if os.path.exists(target_link) or os.path.islink(target_link):
            os.remove(target_link)
        os.symlink(f"/etc/nginx/upstreams/{app_name}-blue.conf", target_link)

        # Create Nginx site config
        nginx_conf = f"""include /etc/nginx/upstreams/{app_name}.conf;

server {{
    listen 80;
    listen [::]:80;
    server_name {domain};

    location /.well-known/acme-challenge/ {{
        root /var/www/html;
    }}

    location / {{
        proxy_pass http://{app_name}_backend;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }}
}}
"""
        with open(f"/etc/nginx/sites-available/{app_name}.conf", "w") as f:
            f.write(nginx_conf)

        site_link = f"/etc/nginx/sites-enabled/{app_name}.conf"
        if not os.path.exists(site_link) and not os.path.islink(site_link):
            os.symlink(f"/etc/nginx/sites-available/{app_name}.conf", site_link)

        run_cmd("nginx -t && systemctl reload nginx")

        # Generate GitHub Actions workflow
        workflow_yaml = f"""name: Deploy {app_name}

on:
  push:
    branches: [main, master]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  deploy:
    uses: 0M3REXE/to-do_vps/.github/workflows/deploy-zero-downtime.yml@main
    with:
      app-name: {app_name}
      binary-name: {app_name}
      domain: {domain}
      port-blue: '{port_blue}'
      port-green: '{port_green}'
      health-path: '/healthz'
      build-css: {'true' if stack == 'rust' else 'false'}
    secrets:
      VPS_HOST: ${{{{ secrets.VPS_HOST }}}}
      VPS_SSH_PORT: ${{{{ secrets.VPS_SSH_PORT }}}}
      VPS_SSH_KEY: ${{{{ secrets.VPS_SSH_KEY }}}}
      VPS_KNOWN_HOSTS: ${{{{ secrets.VPS_KNOWN_HOSTS }}}}
      APP_ENV: ${{{{ secrets.APP_ENV }}}}
"""
        return True, {
            "app_name": app_name,
            "port_blue": port_blue,
            "port_green": port_green,
            "workflow_yaml": workflow_yaml
        }
    else:
        # Piku setup
        run_cmd(f"sudo -iu piku python3 /home/piku/piku.py apps:create {app_name}")
        return True, {"app_name": app_name, "message": f"Piku app '{app_name}' created."}

# ==============================================================================
# HTTP Request Handler
# ==============================================================================
class DashboardHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass # Quiet console

    def send_auth_challenge(self):
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="Micro-PaaS Dashboard"')
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"Unauthorized")

    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode("utf-8"))

    def do_GET(self):
        if not check_auth(self.headers):
            return self.send_auth_challenge()

        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path

        if path == "/" or path == "/index.html":
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(DASHBOARD_HTML.encode("utf-8"))
            return

        elif path == "/api/status":
            stats = get_system_stats()
            self.send_json(stats)
            return

        elif path == "/api/apps":
            bg_apps = get_blue_green_apps()
            piku_apps = get_piku_apps()
            self.send_json({"blue_green": bg_apps, "piku": piku_apps})
            return

        elif path.startswith("/api/apps/") and path.endswith("/env"):
            parts = path.strip("/").split("/")
            app_name = parts[2]
            engine = urllib.parse.parse_qs(parsed.query).get("engine", ["blue_green"])[0]
            vars_list = read_env_file(app_name, engine)
            self.send_json({"app": app_name, "vars": vars_list})
            return

        elif path.startswith("/api/apps/") and path.endswith("/logs"):
            parts = path.strip("/").split("/")
            app_name = parts[2]
            engine = urllib.parse.parse_qs(parsed.query).get("engine", ["blue_green"])[0]
            logs = get_app_logs(app_name, engine)
            self.send_json({"app": app_name, "logs": logs})
            return

        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        if not check_auth(self.headers):
            return self.send_auth_challenge()

        content_length = int(self.headers.get("Content-Length", 0))
        post_data = self.rfile.read(content_length)
        data = {}
        if post_data:
            try:
                data = json.loads(post_data.decode("utf-8"))
            except Exception: pass

        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path

        if path == "/api/apps/action":
            app_name = data.get("app")
            engine = data.get("engine", "blue_green")
            action = data.get("action")

            if engine == "blue_green":
                upstream_link = f"/etc/nginx/upstreams/{app_name}.conf"
                slot = "green"
                if os.path.islink(upstream_link) and "blue" in os.readlink(upstream_link):
                    slot = "blue"

                if action == "restart":
                    run_cmd(f"systemctl restart {app_name}-{slot}")
                elif action == "stop":
                    run_cmd(f"systemctl stop {app_name}-{slot}")
                elif action == "start":
                    run_cmd(f"systemctl start {app_name}-{slot}")
                elif action == "switch":
                    # run switch.sh
                    run_cmd(f"/opt/apps/{app_name}/bin/switch.sh {app_name} 4001 4002 /healthz domain.com")
            else:
                if action == "restart":
                    run_cmd(f"sudo -iu piku python3 /home/piku/piku.py restart {app_name}")
                elif action == "stop":
                    run_cmd(f"sudo -iu piku python3 /home/piku/piku.py stop {app_name}")

            self.send_json({"success": True, "action": action})
            return

        elif path == "/api/apps/save-env":
            app_name = data.get("app")
            engine = data.get("engine", "blue_green")
            vars_list = data.get("vars", [])
            save_env_file(app_name, engine, vars_list)
            self.send_json({"success": True})
            return

        elif path == "/api/apps/create":
            ok, res = create_new_app(data)
            if ok:
                self.send_json({"success": True, "data": res})
            else:
                self.send_json({"success": False, "error": res}, status=400)
            return

        self.send_response(404)
        self.end_headers()

# ==============================================================================
# Modern Embedded Web Dashboard UI
# ==============================================================================
DASHBOARD_HTML = """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <base href="/admin/">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Micro-PaaS Dashboard</title>
  <style>
    :root {
      --bg-dark: #0a0c10;
      --card-bg: #11141a;
      --card-border: #222734;
      --text-main: #f0f3f6;
      --text-dim: #8b949e;
      --accent: #3b82f6;
      --accent-hover: #2563eb;
      --green: #10b981;
      --red: #ef4444;
      --yellow: #f59e0b;
      --slot-blue: #0284c7;
      --slot-green: #059669;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
    body { background-color: var(--bg-dark); color: var(--text-main); min-height: 100vh; display: flex; flex-direction: column; }
    header { background: var(--card-bg); border-bottom: 1px solid var(--card-border); padding: 16px 24px; display: flex; justify-content: space-between; align-items: center; }
    .brand { font-size: 1.25rem; font-weight: 700; display: flex; align-items: center; gap: 8px; }
    .brand span { color: var(--accent); }
    .nav-tabs { display: flex; gap: 8px; }
    .tab-btn { background: transparent; border: 1px solid transparent; color: var(--text-dim); padding: 8px 16px; border-radius: 6px; cursor: pointer; font-size: 0.9rem; font-weight: 500; transition: all 0.2s; }
    .tab-btn.active, .tab-btn:hover { background: var(--card-border); color: #fff; }
    main { max-width: 1200px; width: 100%; margin: 24px auto; padding: 0 16px; flex: 1; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 16px; margin-bottom: 24px; }
    .card { background: var(--card-bg); border: 1px solid var(--card-border); border-radius: 8px; padding: 20px; }
    .card-title { font-size: 0.85rem; color: var(--text-dim); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 8px; }
    .card-val { font-size: 1.6rem; font-weight: 700; }
    .progress-bar { background: var(--card-border); border-radius: 4px; height: 8px; overflow: hidden; margin-top: 10px; }
    .progress-fill { height: 100%; background: var(--accent); border-radius: 4px; transition: width 0.3s; }
    .badge { display: inline-flex; align-items: center; gap: 4px; padding: 4px 8px; border-radius: 4px; font-size: 0.75rem; font-weight: 600; text-transform: uppercase; }
    .badge-green { background: rgba(16, 185, 129, 0.15); color: var(--green); border: 1px solid rgba(16, 185, 129, 0.3); }
    .badge-blue { background: rgba(2, 132, 199, 0.15); color: var(--slot-blue); border: 1px solid rgba(2, 132, 199, 0.3); }
    .badge-gray { background: rgba(139, 148, 158, 0.15); color: var(--text-dim); border: 1px solid var(--card-border); }
    .btn { background: var(--accent); color: #fff; border: none; padding: 8px 14px; border-radius: 6px; cursor: pointer; font-size: 0.85rem; font-weight: 600; display: inline-flex; align-items: center; gap: 6px; transition: background 0.2s; }
    .btn:hover { background: var(--accent-hover); }
    .btn-secondary { background: var(--card-border); color: #fff; }
    .btn-secondary:hover { background: #30363d; }
    .btn-danger { background: rgba(239, 68, 68, 0.15); color: var(--red); border: 1px solid rgba(239, 68, 68, 0.3); }
    .btn-danger:hover { background: var(--red); color: #fff; }
    .app-card { background: var(--card-bg); border: 1px solid var(--card-border); border-radius: 8px; padding: 20px; margin-bottom: 16px; display: flex; flex-direction: column; gap: 16px; }
    .app-header { display: flex; justify-content: space-between; align-items: center; }
    .app-name { font-size: 1.25rem; font-weight: 700; display: flex; align-items: center; gap: 10px; }
    .app-meta { font-size: 0.85rem; color: var(--text-dim); display: flex; gap: 16px; }
    .app-actions { display: flex; gap: 8px; flex-wrap: wrap; }
    .table-container { width: 100%; overflow-x: auto; margin-top: 12px; }
    table { width: 100%; border-collapse: collapse; text-align: left; font-size: 0.9rem; }
    th { padding: 10px 12px; border-bottom: 1px solid var(--card-border); color: var(--text-dim); font-weight: 600; }
    td { padding: 10px 12px; border-bottom: 1px solid var(--card-border); }
    input, select, textarea { background: var(--bg-dark); border: 1px solid var(--card-border); color: #fff; padding: 8px 12px; border-radius: 6px; font-size: 0.9rem; width: 100%; outline: none; }
    input:focus, select:focus, textarea:focus { border-color: var(--accent); }
    .code-box { background: var(--bg-dark); border: 1px solid var(--card-border); border-radius: 6px; padding: 16px; font-family: monospace; font-size: 0.85rem; color: #79c0ff; overflow-x: auto; white-space: pre; max-height: 400px; }
    .modal-overlay { position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.7); display: flex; align-items: center; justify-content: center; z-index: 100; opacity: 0; pointer-events: none; transition: opacity 0.2s; }
    .modal-overlay.active { opacity: 1; pointer-events: auto; }
    .modal-card { background: var(--card-bg); border: 1px solid var(--card-border); border-radius: 8px; width: 90%; max-width: 700px; max-height: 90vh; overflow-y: auto; padding: 24px; display: flex; flex-direction: column; gap: 16px; }
    .secret-val { font-family: monospace; letter-spacing: 0.1em; }
    .masked { filter: blur(4px); user-select: none; transition: filter 0.2s; }
    .masked.revealed { filter: none; }
  </style>
</head>
<body>
  <header>
    <div class="brand">🚀 Micro-PaaS <span>Dashboard</span></div>
    <div class="nav-tabs">
      <button class="tab-btn active" onclick="switchTab('overview')">Overview</button>
      <button class="tab-btn" onclick="switchTab('apps')">Apps</button>
      <button class="tab-btn" onclick="switchTab('new-app')">+ New App & CI/CD</button>
    </div>
  </header>

  <main>
    <!-- TAB: OVERVIEW -->
    <section id="tab-overview">
      <div class="grid">
        <div class="card">
          <div class="card-title">Memory Usage</div>
          <div class="card-val" id="mem-val">...</div>
          <div class="progress-bar"><div class="progress-fill" id="mem-bar" style="width: 0%"></div></div>
        </div>
        <div class="card">
          <div class="card-title">Disk Storage</div>
          <div class="card-val" id="disk-val">...</div>
          <div class="progress-bar"><div class="progress-fill" id="disk-bar" style="width: 0%"></div></div>
        </div>
        <div class="card">
          <div class="card-title">CPU Load (1m, 5m, 15m)</div>
          <div class="card-val" id="cpu-val">...</div>
          <div style="font-size: 0.85rem; color: var(--text-dim); margin-top: 8px;">Uptime: <span id="uptime-val">...</span></div>
        </div>
        <div class="card">
          <div class="card-title">Detected Deployment Engines</div>
          <div style="display: flex; flex-direction: column; gap: 8px; margin-top: 10px;">
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span>Zero-Downtime Blue/Green</span>
              <span class="badge badge-green" id="engine-bg">ACTIVE</span>
            </div>
            <div style="display: flex; justify-content: space-between; align-items: center;">
              <span>Piku Engine</span>
              <span class="badge badge-gray" id="engine-piku">DETECTED</span>
            </div>
          </div>
        </div>
      </div>
      <div class="card">
        <div class="card-title">Firewall Status (UFW)</div>
        <pre class="code-box" id="fw-box" style="max-height: 150px;">Loading...</pre>
      </div>
    </section>

    <!-- TAB: APPS -->
    <section id="tab-apps" style="display: none;">
      <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px;">
        <h2>Deployed Applications</h2>
        <button class="btn btn-secondary" onclick="loadApps()">🔄 Refresh</button>
      </div>
      <div id="apps-list">Loading apps...</div>
    </section>

    <!-- TAB: NEW APP & CI/CD -->
    <section id="tab-new-app" style="display: none;">
      <div class="card">
        <h2 style="margin-bottom: 16px;">Provision New App & Generate CI/CD</h2>
        <div style="display: flex; flex-direction: column; gap: 16px; max-width: 500px;">
          <div>
            <label style="font-size: 0.85rem; color: var(--text-dim); display: block; margin-bottom: 4px;">App Name (e.g. notes, api)</label>
            <input type="text" id="new-app-name" placeholder="notes">
          </div>
          <div>
            <label style="font-size: 0.85rem; color: var(--text-dim); display: block; margin-bottom: 4px;">Public Domain (e.g. notes.duckdns.org)</label>
            <input type="text" id="new-app-domain" placeholder="notes.duckdns.org">
          </div>
          <div>
            <label style="font-size: 0.85rem; color: var(--text-dim); display: block; margin-bottom: 4px;">Language / Stack</label>
            <select id="new-app-stack">
              <option value="rust">Rust (Axum / Actix)</option>
              <option value="go">Go (Golang)</option>
              <option value="node">Node.js (Standalone / Express)</option>
              <option value="python">Python</option>
            </select>
          </div>
          <div>
            <label style="font-size: 0.85rem; color: var(--text-dim); display: block; margin-bottom: 4px;">Deployment Mode</label>
            <select id="new-app-engine">
              <option value="blue_green">Zero-Downtime Blue/Green (Recommended)</option>
              <option value="piku">Piku (Single Worker)</option>
            </select>
          </div>
          <button class="btn" onclick="submitCreateApp()">🚀 Provision & Generate CI/CD Workflow</button>
        </div>

        <div id="new-app-result" style="display: none; margin-top: 24px;">
          <h3 style="color: var(--green); margin-bottom: 8px;">✅ App Provisioned on VPS!</h3>
          <p style="font-size: 0.85rem; color: var(--text-dim); margin-bottom: 12px;">Copy this file into your new repository at <code>.github/workflows/deploy.yml</code>:</p>
          <pre class="code-box" id="result-workflow"></pre>
        </div>
      </div>
    </section>
  </main>

  <!-- MODAL: SECRETS MANAGER -->
  <div class="modal-overlay" id="secrets-modal">
    <div class="modal-card">
      <div style="display: flex; justify-content: space-between; align-items: center;">
        <h3 id="modal-app-name">Secrets Manager</h3>
        <button class="btn btn-secondary" onclick="closeModal('secrets-modal')">✕</button>
      </div>
      <p style="font-size: 0.85rem; color: var(--text-dim);">Edit environment variables directly. Saving applies immediately and restarts the active slot.</p>
      <div style="display: flex; gap: 8px; align-items: center;">
        <button class="btn btn-secondary" onclick="toggleSecretMask()">👁️ Toggle Reveal</button>
        <button class="btn btn-secondary" onclick="addSecretRow()">+ Add Variable</button>
      </div>
      <div class="table-container">
        <table id="secrets-table">
          <thead>
            <tr>
              <th style="width: 35%;">Key</th>
              <th>Value</th>
              <th style="width: 50px;"></th>
            </tr>
          </thead>
          <tbody id="secrets-tbody"></tbody>
        </table>
      </div>
      <div style="display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px;">
        <button class="btn btn-secondary" onclick="closeModal('secrets-modal')">Cancel</button>
        <button class="btn" onclick="saveSecrets()">💾 Save & Apply</button>
      </div>
    </div>
  </div>

  <!-- MODAL: LOGS VIEWER -->
  <div class="modal-overlay" id="logs-modal">
    <div class="modal-card" style="max-width: 900px;">
      <div style="display: flex; justify-content: space-between; align-items: center;">
        <h3 id="logs-app-title">Application Logs</h3>
        <button class="btn btn-secondary" onclick="closeModal('logs-modal')">✕</button>
      </div>
      <pre class="code-box" id="logs-content" style="max-height: 500px; height: 500px;">Loading logs...</pre>
    </div>
  </div>

  <script>
    let currentApp = null;
    let currentEngine = 'blue_green';
    let secretsMasked = true;

    function switchTab(tab) {
      document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
      document.querySelectorAll('main > section').forEach(s => s.style.display = 'none');
      document.getElementById('tab-' + tab).style.display = 'block';
      event.target.classList.add('active');
      if (tab === 'apps') loadApps();
    }

    async function loadStatus() {
      try {
        const res = await fetch('api/status');
        const data = await res.json();
        document.getElementById('mem-val').innerText = `${data.ram.used_mb} MB / ${data.ram.total_mb} MB`;
        document.getElementById('mem-bar').style.width = `${data.ram.pct}%`;
        document.getElementById('disk-val').innerText = `${data.disk.used_gb} GB / ${data.disk.total_gb} GB (${data.disk.pct}%)`;
        document.getElementById('disk-bar').style.width = `${data.disk.pct}%`;
        document.getElementById('cpu-val').innerText = data.cpu_load.join(', ');
        document.getElementById('uptime-val').innerText = data.uptime;
        document.getElementById('fw-box').innerText = data.firewall.raw;
      } catch (err) {
        console.error(err);
      }
    }

    async function loadApps() {
      try {
        const res = await fetch('api/apps');
        const data = await res.json();
        const container = document.getElementById('apps-list');
        container.innerHTML = '';

        const allApps = [...data.blue_green, ...data.piku];
        if (allApps.length === 0) {
          container.innerHTML = '<p style="color: var(--text-dim);">No apps detected.</p>';
          return;
        }

        allApps.forEach(app => {
          const isBg = (app.engine === 'blue_green');
          const slotBadge = isBg 
            ? `<span class="badge ${app.active_slot === 'green' ? 'badge-green' : 'badge-blue'}">Slot: ${app.active_slot} (Port ${app.current_port})</span>`
            : `<span class="badge badge-gray">Piku (Port ${app.current_port})</span>`;

          const healthBadge = app.healthy 
            ? `<span class="badge badge-green">Healthy 200</span>` 
            : `<span class="badge badge-gray">Offline / Stopped</span>`;

          const card = document.createElement('div');
          card.className = 'app-card';
          card.innerHTML = `
            <div class="app-header">
              <div class="app-name">
                ${app.name}
                ${slotBadge}
                ${healthBadge}
              </div>
              <div class="app-actions">
                <button class="btn btn-secondary" onclick="openLogs('${app.name}', '${app.engine}')">📜 Logs</button>
                <button class="btn btn-secondary" onclick="openSecrets('${app.name}', '${app.engine}')">🔐 Secrets</button>
                <button class="btn btn-secondary" onclick="triggerAction('${app.name}', '${app.engine}', 'restart')">🔄 Restart</button>
                ${isBg ? `<button class="btn btn-secondary" onclick="triggerAction('${app.name}', '${app.engine}', 'switch')">🔀 Switch Slot</button>` : ''}
              </div>
            </div>
            <div class="app-meta">
              <span>Engine: <b>${isBg ? 'Zero-Downtime Blue/Green' : 'Piku'}</b></span>
              <span>Root: <code>${app.path}</code></span>
            </div>
          `;
          container.appendChild(card);
        });
      } catch (err) {
        console.error(err);
      }
    }

    async function triggerAction(app, engine, action) {
      if (!confirm(`Are you sure you want to ${action} ${app}?`)) return;
      try {
        await fetch('api/apps/action', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ app, engine, action })
        });
        alert(`Action ${action} triggered!`);
        loadApps();
      } catch (err) {
        alert('Action failed: ' + err);
      }
    }

    async function openSecrets(app, engine) {
      currentApp = app;
      currentEngine = engine;
      document.getElementById('modal-app-name').innerText = `Secrets: ${app} (${engine})`;
      const tbody = document.getElementById('secrets-tbody');
      tbody.innerHTML = '<tr><td colspan="3">Loading...</td></tr>';
      document.getElementById('secrets-modal').classList.add('active');

      const res = await fetch(`api/apps/${app}/env?engine=${engine}`);
      const data = await res.json();
      tbody.innerHTML = '';
      data.vars.forEach(v => {
        addSecretRow(v.key, v.value);
      });
    }

    function addSecretRow(key = '', val = '') {
      const tbody = document.getElementById('secrets-tbody');
      const tr = document.createElement('tr');
      tr.innerHTML = `
        <td><input type="text" class="secret-key" value="${key}" placeholder="KEY"></td>
        <td><input type="text" class="secret-val ${secretsMasked ? 'masked' : ''}" value="${val}" placeholder="VALUE"></td>
        <td><button class="btn btn-danger" onclick="this.closest('tr').remove()">✕</button></td>
      `;
      tbody.appendChild(tr);
    }

    function toggleSecretMask() {
      secretsMasked = !secretsMasked;
      document.querySelectorAll('.secret-val').forEach(input => {
        if (secretsMasked) input.classList.add('masked');
        else input.classList.remove('masked');
      });
    }

    async function saveSecrets() {
      const rows = document.querySelectorAll('#secrets-tbody tr');
      const vars = [];
      rows.forEach(r => {
        const k = r.querySelector('.secret-key')?.value.trim();
        const v = r.querySelector('.secret-val')?.value.trim();
        if (k) vars.push({ key: k, value: v });
      });

      try {
        await fetch('api/apps/save-env', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ app: currentApp, engine: currentEngine, vars })
        });
        alert('Secrets saved and active slot reloaded!');
        closeModal('secrets-modal');
        loadApps();
      } catch (err) {
        alert('Failed saving secrets: ' + err);
      }
    }

    async function openLogs(app, engine) {
      document.getElementById('logs-app-title').innerText = `Logs: ${app}`;
      document.getElementById('logs-content').innerText = 'Streaming logs...';
      document.getElementById('logs-modal').classList.add('active');

      const res = await fetch(`api/apps/${app}/logs?engine=${engine}`);
      const data = await res.json();
      document.getElementById('logs-content').innerText = data.logs || 'No log output.';
    }

    function closeModal(id) {
      document.getElementById(id).classList.remove('active');
    }

    async function submitCreateApp() {
      const app_name = document.getElementById('new-app-name').value.trim();
      const domain = document.getElementById('new-app-domain').value.trim();
      const stack = document.getElementById('new-app-stack').value;
      const engine = document.getElementById('new-app-engine').value;

      if (!app_name || !domain) {
        alert('Please fill in both app name and domain.');
        return;
      }

      try {
        const res = await fetch('api/apps/create', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ app_name, domain, stack, engine })
        });
        const data = await res.json();
        if (data.success) {
          document.getElementById('new-app-result').style.display = 'block';
          document.getElementById('result-workflow').innerText = data.data.workflow_yaml || data.data.message;
        } else {
          alert('Error: ' + data.error);
        }
      } catch (err) {
        alert('Failed creating app: ' + err);
      }
    }

    // Auto load on start
    loadStatus();
    setInterval(loadStatus, 10000);
  </script>
</body>
</html>
"""

if __name__ == "__main__":
    os.makedirs(CONFIG_DIR, exist_ok=True)
    server = HTTPServer((HOST, PORT), DashboardHandler)
    print(f"🚀 Micro-PaaS Dashboard listening on http://{HOST}:{PORT}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass

