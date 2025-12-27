import http from "k6/http";
import { check, sleep } from "k6";
import { Trend } from "k6/metrics";

// Test configuration
export let options = {
  stages: [
    { duration: "30s", target: 10 }, // Ramp up to 10 users over 30s
    { duration: "1m", target: 10 }, // Stay at 10 users for 1 minute
    { duration: "30s", target: 50 }, // Ramp up to 50 users over 30s
    { duration: "1m", target: 50 }, // Stay at 50 users for 1 minute
    { duration: "30s", target: 100 }, // Ramp up to 100 users over 30s
    { duration: "2m", target: 100 }, // Stay at 100 users for 2 minutes
    { duration: "30s", target: 0 }, // Ramp down to 0 users
  ],
  thresholds: {
    http_req_duration: ["p(95)<" + (__ENV.THRESHOLD_MS || 500)],
    http_req_failed: ["rate<0.1"],
  },
  summaryTrendStats: ["min", "med", "p(90)", "p(95)", "max"],
};

const BASE_URL = "https://softagen.com";

// Custom trends for deeper analysis
const t_connect = new Trend("timings_connect", true);
const t_tls = new Trend("timings_tls", true);
const t_ttfb = new Trend("timings_ttfb", true); // waiting == TTFB
const t_receive = new Trend("timings_receive", true);
const t_total = new Trend("timings_total", true);

export default function () {
  // Test the root endpoint
  const response = http.get(`${BASE_URL}/`, {
    headers: {
      "User-Agent": "k6-aiwebengine-load-test"
    },
  });

  // Record timing breakdowns (coalesce nulls from connection reuse)
  const tm = (response && typeof response === "object" && response.timings) ? response.timings : null;
  if (tm) {
    t_connect.add(Number(tm.connecting || 0));
    t_tls.add(Number(tm.tls_handshaking || 0));
    t_ttfb.add(Number(tm.waiting || 0));
    t_receive.add(Number(tm.receiving || 0));
    t_total.add(Number(tm.duration || 0));
  }

  // More robust content checks
  const headers = response.headers || {};
  const contentType = headers["Content-Type"] || headers["content-type"] || "";
  const isHtml = typeof contentType === "string" && contentType.includes("text/html");
  const hasHtmlTag = typeof response.body === "string" && response.body.toLowerCase().includes("<html");

  check(response, {
    "status is 200": (r) => r.status === 200,
    "response time < threshold": (r) => r.timings.duration < (Number(__ENV.THRESHOLD_MS) || 500),
    "content-type is text/html": (_r) => isHtml,
    "contains <html> tag": (_r) => hasHtmlTag,
  });

  // Simulate real user behavior with random sleep
  sleep(Math.random() * 2 + 1);
}
