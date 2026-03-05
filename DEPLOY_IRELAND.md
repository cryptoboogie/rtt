# Deploy to AWS Ireland (eu-west-1)

## 0. Why Ireland?

Polymarket's CLOB matching engine runs in AWS **eu-west-2 (London)**. Ireland (eu-west-1) is the nearest non-blocked AWS region. Expected network RTT: **1-3ms** Ireland→London. This is where serious PM bots run.

Blocked regions: US, UK, Netherlands (as of early 2026). Safe alternatives: Ireland, Switzerland, Austria, Germany.

---

## 1. AWS Account Setup (if new)

Go to https://aws.amazon.com/ → Create Account. You need a credit card. New accounts get 12 months of Free Tier (t2.micro/t3.micro included).

---

## 2. Security Configuration (IMPORTANT — you have sensitive keys)

Your `.env` contains private keys and API secrets. Lock this down properly.

### 2a. Create a Dedicated IAM User (don't use root)

1. Go to **IAM** → **Users** → **Create user**
2. Username: `rtt-deployer`
3. Attach policy: `AmazonEC2FullAccess` (or narrower if you want)
4. Create access keys only if you plan to use AWS CLI (optional — console is fine)
5. Enable **MFA** on both root and this IAM user: IAM → Users → Security credentials → MFA device → Assign

### 2b. Create a Key Pair

1. Go to **EC2** → **Key Pairs** (left sidebar, under Network & Security)
2. Click **Create key pair**
3. Name: `rtt-ireland`
4. Key pair type: **RSA**
5. Private key format: **.pem**
6. Click **Create** — downloads `rtt-ireland.pem` to your machine
7. Secure it immediately:

```bash
# Move to a safe location
mv ~/Downloads/rtt-ireland.pem ~/.ssh/rtt-ireland.pem
chmod 400 ~/.ssh/rtt-ireland.pem
```

### 2c. Create a Security Group (firewall)

1. Make sure you're in **eu-west-1** region (top-right dropdown → "Europe (Ireland)")
2. Go to **EC2** → **Security Groups** → **Create security group**
3. Name: `rtt-ssh-only`
4. Description: `SSH from my IP only`
5. VPC: leave default

**Inbound rules** — click "Add rule":

| Type | Port | Source | Description |
|------|------|--------|-------------|
| SSH | 22 | My IP (auto-fills your current IP) | SSH access |

That's it. **No other inbound ports**. The bot only makes outbound HTTPS connections. No inbound web traffic needed.

**Outbound rules**: Leave default (all outbound allowed). The bot needs to reach:
- `clob.polymarket.com:443` (CLOB API)
- `ws-subscriptions-clob.polymarket.com:443` (WebSocket, if running full pipeline)
- `github.com:443` (git clone)

6. Click **Create security group**

### 2d. On the EC2 Instance (after launch)

Your `.env` file with POLY_PRIVATE_KEY lives **only on this box**. Additional hardening:

```bash
# .env is owner-read only (no other users can see it)
chmod 600 ~/rtt/.env

# Disable password auth (key-only SSH) — Ubuntu 24.04 does this by default
# Verify with:
grep PasswordAuthentication /etc/ssh/sshd_config
# Should show: PasswordAuthentication no

# Enable automatic security updates
sudo apt install -y unattended-upgrades
sudo dpkg-reconfigure -plow unattended-upgrades
# Select "Yes" when prompted
```

### 2e. What NOT to do

- **Don't** commit `.env` to git (it's already in `.gitignore`)
- **Don't** open ports 80/443/8080 inbound — you don't need them
- **Don't** use the AWS root account for day-to-day work
- **Don't** paste your private key into any web form or chat
- **Don't** leave the instance running 24/7 if you're just testing — stop it when done

---

## 3. Launch EC2 Instance

1. Go to **EC2** → **Launch Instance** (make sure region is **eu-west-1** top-right)

2. Fill in:

| Setting | Value |
|---------|-------|
| Name | `rtt-bot` |
| AMI | Ubuntu Server 24.04 LTS — pick **64-bit (x86)** for t3, or **64-bit (Arm)** for t4g |
| Instance type | `t3.small` (2 vCPU, 2 GB RAM, ~$15/mo) — recommended for builds |
| Key pair | `rtt-ireland` (the one you created above) |
| Security group | Select existing → `rtt-ssh-only` |
| Storage | 20 GiB gp3 |

3. Click **Launch instance**

4. Wait ~30 seconds, then go to **Instances** → click your instance → copy the **Public IPv4 address**

> **Cost note**: t3.small = $0.0208/hr. Stop the instance when not using it (you're only charged for EBS storage when stopped, ~$1.60/mo for 20GB). For long-running production use, consider a Reserved Instance or Savings Plan for ~40% discount.

---

## 4. SSH In

```bash
ssh -i ~/.ssh/rtt-ireland.pem ubuntu@<PUBLIC_IP>
```

First connection will ask to confirm the host fingerprint — type `yes`.

If you get "Permission denied": make sure the key file is `chmod 400` and you're using `ubuntu@` (not `root@`).

> **Tip**: Add to `~/.ssh/config` for convenience:
> ```
> Host rtt
>     HostName <PUBLIC_IP>
>     User ubuntu
>     IdentityFile ~/.ssh/rtt-ireland.pem
> ```
> Then just: `ssh rtt`

---

## 5. Install Dependencies (run these on the EC2 box)

Copy-paste this entire block:

```bash
# Update package list and install build tools
sudo apt update && sudo apt upgrade -y
sudo apt install -y build-essential pkg-config libssl-dev git curl

# Install Rust (accept defaults — option 1)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Load Rust into current shell
source "$HOME/.cargo/env"

# Verify
rustc --version   # expect: rustc 1.8x.x
cargo --version   # expect: cargo 1.8x.x
```

Total time: ~1 minute.

---

## 6. Clone and Build

```bash
cd ~
git clone https://github.com/cryptoboogie/rtt.git
cd rtt

# Release build — CRITICAL for real latency numbers
# Debug builds are 5-10x slower on the hot path
cargo build --release --workspace
```

Build takes **3-5 minutes** on t3.small (first time only — subsequent builds are incremental and fast).

If the build fails with out-of-memory on t3.micro (1 GB RAM), either:
- Use t3.small (2 GB) instead, or
- Add swap: `sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile`

---

## 7. Set Credentials

```bash
cd ~/rtt

# Create .env file — paste your real values
cat > .env << 'EOF'
POLY_PRIVATE_KEY=0xyour_private_key_here
POLY_ADDRESS=0xyour_address_here
POLY_API_KEY=your_api_key_here
POLY_SECRET=your_secret_here
POLY_PASSPHRASE=your_passphrase_here
EOF

# Lock permissions — only your user can read this file
chmod 600 .env

# Verify it looks right (be careful — this prints your secrets to terminal)
cat .env
```

---

## 8. Verify Network Path (before spending money)

```bash
# Check ping to CLOB (should be 1-3ms from Ireland)
ping -c 5 clob.polymarket.com

# Check which Cloudflare POP you hit (want LHR = London)
curl -sI https://clob.polymarket.com/ | grep -i cf-ray
# Example output: cf-ray: 8a1234567890-LHR
#                                      ^^^ this is the POP code

# Check if you're geoblocked (should get a JSON response, not a block page)
curl -s https://clob.polymarket.com/time | head -c 200
```

Expected results:
- Ping: **1-3ms** (Ireland → London)
- cf-ray: ends in `LHR` (London Heathrow datacenter)
- /time: returns a JSON timestamp (not an error page)

If ping is >5ms or POP is not LHR, something is wrong with routing.

---

## 9. Run the Test Trade

This sends a real signed order to the CLOB. It will fail with "insufficient balance" (expected — proves the full pipeline works).

```bash
cd ~/rtt

# Load env vars and run the end-to-end test
set -a && source .env && set +a && \
  cargo test --release -p rtt-core test_clob_end_to_end_pipeline -- --ignored --nocapture
```

### What this does:

1. Warms an H2 connection to `clob.polymarket.com`
2. Signs a real order (EIP-712 signature)
3. Computes HMAC L2 auth headers
4. Sends the order on the warm connection
5. Prints the HTTP response body (expect `"insufficient balance"` or similar 400 error)
6. Prints all **8 timestamp checkpoints** + derived latency metrics
7. Shows the Cloudflare POP code

### What to look for in the output:

```
=== CLOB End-to-End Pipeline ===

--- Timestamp Checkpoints ---
t_trigger_rx:      1234567890    (trigger received)
t_dispatch_q:      1234567891    (dequeued)
t_exec_start:      1234567892    (execution began)
t_buf_ready:       1234567900    (request bytes ready)
t_write_begin:     1234567901    (started writing to connection)
t_write_end:       1234567910    (H2 frame submitted to kernel)
t_first_resp_byte: 1234570000    (first response byte from server)
t_headers_done:    1234570100    (response fully received)

--- Derived Metrics ---
trigger_to_wire:   <100 us       (YOUR code's overhead — what you control)
write_duration:    <50 us        (H2 frame submission to kernel)
warm_ttfb:         2-5 ms        (network RTT Ireland→London — physics)
pop:               LHR           (confirms London Cloudflare routing)

--- Response Body ---
{"error": "insufficient balance"} or similar
```

### Target numbers from Ireland:

| Metric | Target | What it means |
|--------|--------|---------------|
| trigger_to_wire | <100 us | Time from trigger to H2 frame on wire (your code) |
| write_duration | <50 us | Time to submit H2 frame to kernel buffer |
| warm_ttfb | 1-5 ms | Network round-trip Ireland→London (physics limit) |
| pop | LHR | Cloudflare London POP (closest to CLOB in eu-west-2) |

If `warm_ttfb` > 10ms → routing issue, check security group or try a different AZ.
If `pop` is not `LHR` → Cloudflare is routing you to a farther POP, unusual from eu-west-1.

---

## 10. Run the Full Pipeline (optional — when ready for live trading)

Edit `config.toml` to set your strategy params, then:

```bash
cd ~/rtt

# Edit config — set dry_run=false only when ready for real orders
nano config.toml

# Run the full pipeline
set -a && source .env && set +a && \
  cargo run --release -p pm-executor -- config.toml
```

This starts the full WebSocket → Strategy → Execution pipeline. Use `dry_run = true` first to verify everything connects without spending money.

---

## 11. Running as a Background Service (optional)

To keep the bot running after you disconnect SSH:

```bash
# Option A: tmux (simple, good for testing)
sudo apt install -y tmux
tmux new -s rtt
cd ~/rtt && set -a && source .env && set +a && cargo run --release -p pm-executor -- config.toml
# Detach: Ctrl+B then D
# Reattach later: tmux attach -t rtt

# Option B: systemd (production — auto-restarts on crash)
sudo tee /etc/systemd/system/rtt.service << 'EOF'
[Unit]
Description=RTT Polymarket Bot
After=network.target

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/home/ubuntu/rtt
EnvironmentFile=/home/ubuntu/rtt/.env
ExecStart=/home/ubuntu/rtt/target/release/pm-executor /home/ubuntu/rtt/config.toml
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable rtt
sudo systemctl start rtt

# Check status
sudo systemctl status rtt
# View logs
sudo journalctl -u rtt -f
```

---

## Quick Reference Card

```bash
# SSH in
ssh -i ~/.ssh/rtt-ireland.pem ubuntu@<PUBLIC_IP>

# Run test trade (one-shot, expect "insufficient balance")
cd ~/rtt && set -a && source .env && set +a && \
  cargo test --release -p rtt-core test_clob_end_to_end_pipeline -- --ignored --nocapture

# Run full pipeline (dry run)
cd ~/rtt && set -a && source .env && set +a && \
  cargo run --release -p pm-executor -- config.toml

# Check network path
ping -c 3 clob.polymarket.com
curl -sI https://clob.polymarket.com/ | grep cf-ray

# Pull latest code and rebuild
cd ~/rtt && git pull && cargo build --release --workspace

# Stop the instance (from AWS Console or CLI)
# EC2 → Instances → Select → Instance state → Stop instance
```

## Cost

| Instance | Specs | Hourly | Monthly (24/7) |
|----------|-------|--------|----------------|
| t3.micro | 2 vCPU, 1 GB | $0.0104 | ~$8 |
| t3.small | 2 vCPU, 2 GB | $0.0208 | ~$15 |
| t4g.small | 2 vCPU, 2 GB (ARM) | $0.0168 | ~$12 |

**Stop the instance when not testing** — you only pay for EBS storage when stopped (~$1.60/mo for 20GB gp3). You can also set a billing alarm: AWS Budgets → Create budget → $20/month → email alert.
