#!/usr/bin/env node
"use strict";

const express = require("express");
const cors    = require("cors");
const si      = require("systeminformation");
const path    = require("path");

const app  = express();
const PORT = 3000;

app.use(cors());
app.use(express.static(path.join(__dirname)));   // serve dashboard.html, etc.

// ─── helpers ────────────────────────────────────────────────────────────────
function pick(obj, keys) {
  return Object.fromEntries(keys.map(k => [k, obj[k] ?? null]));
}

// ─── /api/stats  (polled every second by the dashboard) ─────────────────────
app.get("/api/stats", async (_req, res) => {
  try {
    const [
      cpuLoad,
      mem,
      disksIO,
      netStats,
      processes,
      battery,
    ] = await Promise.all([
      si.currentLoad(),
      si.mem(),
      si.disksIO(),
      si.networkStats(),            // all interfaces
      si.processes(),
      si.battery().catch(() => ({})),
    ]);

    // ── CPU ─────────────────────────────────────────────────────────────────
    const cpu = {
      loadPercent:  Math.round(cpuLoad.currentLoad * 10) / 10,
      userPercent:  Math.round(cpuLoad.currentLoadUser * 10) / 10,
      sysPercent:   Math.round(cpuLoad.currentLoadSystem * 10) / 10,
      cores: (cpuLoad.cpus || []).map(c => Math.round(c.load * 10) / 10),
    };

    // ── Memory ───────────────────────────────────────────────────────────────
    const memory = {
      totalBytes:  mem.total,
      usedBytes:   mem.used,
      activeBytes: mem.active,
      freeBytes:   mem.free,
      availBytes:  mem.available,
      swapTotal:   mem.swaptotal,
      swapUsed:    mem.swapused,
      usedPercent: Math.round(mem.used / mem.total * 1000) / 10,
    };

    // ── Disk I/O ─────────────────────────────────────────────────────────────
    // disksIO returns cumulative counters; we keep a delta in memory
    const disk = {
      readBps:    disksIO.rIO_sec  ?? 0,   // bytes/s (systeminformation computes the rate)
      writeBps:   disksIO.wIO_sec  ?? 0,
      readOps:    disksIO.rIO      ?? 0,
      writeOps:   disksIO.wIO      ?? 0,
    };

    // ── Network ──────────────────────────────────────────────────────────────
    // Sum across all non-loopback interfaces
    const netArr = Array.isArray(netStats) ? netStats : [netStats];
    const net = netArr
      .filter(n => n.iface && n.iface !== "lo")
      .reduce(
        (acc, n) => ({
          rxBps:        acc.rxBps     + (n.rx_sec   ?? 0),
          txBps:        acc.txBps     + (n.tx_sec   ?? 0),
          rxTotal:      acc.rxTotal   + (n.rx_bytes  ?? 0),
          txTotal:      acc.txTotal   + (n.tx_bytes  ?? 0),
          ifaces:       [...acc.ifaces, n.iface],
        }),
        { rxBps: 0, txBps: 0, rxTotal: 0, txTotal: 0, ifaces: [] }
      );

    // ── Processes ────────────────────────────────────────────────────────────
    const procs = (processes.list || [])
      .filter(p => p.pcpu != null && p.pcpu >= 0)   // drop kernel threads with null cpu
      .sort((a, b) => b.pcpu - a.pcpu)
      .slice(0, 40)
      .map(p => ({
        pid:      p.pid,
        name:     p.name,
        cmd:      p.command || p.name,
        cpu:      Math.round(p.pcpu  * 10) / 10,
        memBytes: Math.round((p.mem_rss || 0) * 1024),
        status:   p.state  || "—",
        user:     p.user   || "—",
      }));

    // ── Battery (optional) ───────────────────────────────────────────────────
    const bat = battery.hasBattery
      ? { hasBattery: true, percent: battery.percent, isCharging: battery.isCharging }
      : { hasBattery: false };

    res.json({ cpu, memory, disk, net, processes: procs, battery: bat, ts: Date.now() });
  } catch (err) {
    console.error("stats error:", err.message);
    res.status(500).json({ error: err.message });
  }
});

// ─── /api/static  (one-shot: CPU info, OS info, GPU) ────────────────────────
app.get("/api/static", async (_req, res) => {
  try {
    const [cpuInfo, osInfo, gpuInfo, fsSize] = await Promise.all([
      si.cpu(),
      si.osInfo(),
      si.graphics(),
      si.fsSize(),
    ]);

    const gpu = (gpuInfo.controllers || []).map(g => pick(g, [
      "model", "vendor", "vram", "vramDynamic", "driverVersion",
    ]));

    const disks = (fsSize || []).map(d => pick(d, [
      "fs", "type", "size", "used", "available", "use", "mount",
    ]));

    res.json({
      cpu: pick(cpuInfo, [
        "manufacturer", "brand", "speed", "speedMax", "cores",
        "physicalCores", "processors", "socket", "cache",
      ]),
      os: pick(osInfo, [
        "platform", "distro", "release", "arch", "hostname", "kernel",
      ]),
      gpu,
      disks,
    });
  } catch (err) {
    console.error("static error:", err.message);
    res.status(500).json({ error: err.message });
  }
});

app.listen(PORT, () => {
  console.log(`System Dashboard backend → http://localhost:${PORT}`);
  console.log(`Dashboard UI            → http://localhost:${PORT}/dashboard.html`);
});
