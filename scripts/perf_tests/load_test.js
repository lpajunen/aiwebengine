import http from 'k6/http';
import { check, sleep } from 'k6';

// Test configuration
export let options = {
  stages: [
    { duration: '30s', target: 10 },   // Ramp up to 10 users over 30s
    { duration: '1m', target: 10 },    // Stay at 10 users for 1 minute
    { duration: '30s', target: 50 },   // Ramp up to 50 users over 30s
    { duration: '1m', target: 50 },    // Stay at 50 users for 1 minute
    { duration: '30s', target: 100 },  // Ramp up to 100 users over 30s
    { duration: '2m', target: 100 },   // Stay at 100 users for 2 minutes
    { duration: '30s', target: 0 },    // Ramp down to 0 users
  ],
  thresholds: {
    http_req_duration: ['p(95)<500'], // 95% of requests should be below 500ms
    http_req_failed: ['rate<0.1'],    // Error rate should be below 10%
  },
};

const BASE_URL = 'https://softagen.com';

export default function () {
  // Test the root endpoint (returns HTML welcome page)
  let response = http.get(`${BASE_URL}/`);

  // Basic checks
  check(response, {
    'status is 200': (r) => r.status === 200,
    'response time < 500ms': (r) => r.timings.duration < 500,
    'has HTML content': (r) => r.body.includes('Welcome'),
  });

  // Simulate real user behavior with random sleep
  sleep(Math.random() * 2 + 1); // Sleep 1-3 seconds
}