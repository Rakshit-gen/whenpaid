// k6 load test for the webhook receiver. Every request is a correctly signed,
// unique payout event, so each one does the full path: tenant lookup,
// signature check, inbox insert, payout upsert, job bump, commit.
//
//   k6 run -e SECRET=whsec_... -e BUSINESS=bus_bench bench/webhooks.js
import http from 'k6/http';
import crypto from 'k6/crypto';
import encoding from 'k6/encoding';
import { check } from 'k6';

export const options = {
  scenarios: { ingest: { executor: 'constant-vus', vus: 32, duration: '30s' } },
  summaryTrendStats: ['avg', 'p(50)', 'p(95)', 'p(99)', 'max'],
};

const key = encoding.b64decode(__ENV.SECRET.replace('whsec_', ''));
const business = __ENV.BUSINESS;

export default function () {
  const id = `msg_${__VU}_${__ITER}`;
  const ts = Math.floor(Date.now() / 1000).toString();
  const body = JSON.stringify({
    business_id: business, type: 'payout.success', timestamp: new Date().toISOString(),
    data: { payload_type: 'Payout', payout_id: `pyt_${__VU}_${__ITER % 50}`, amount: 49500, currency: 'USD', fee: 500,
            created_at: '2026-11-18T00:00:04Z', status: 'success', business_id: business },
  });
  const sig = crypto.hmac('sha256', key, `${id}.${ts}.${body}`, 'base64');
  const r = http.post(`http://127.0.0.1:8099/webhooks/${business}`, body, {
    headers: { 'content-type': 'application/json', 'webhook-id': id, 'webhook-timestamp': ts, 'webhook-signature': `v1,${sig}` },
  });
  check(r, { stored: (res) => res.status === 200 && res.json('stored') === true });
}
