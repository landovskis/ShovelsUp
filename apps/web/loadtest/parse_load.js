// IMP-REQ-002-10 / TC-REQ-002-5: sustained parsing throughput across mixed
// formats (HTML/PDF/plain-text).
//
// Drives reprocessing across a pre-seeded batch of source_documents spanning
// all three content types via the admin reprocess endpoint (the only
// HTTP-reachable trigger for parse_and_store — see fetch_load.js's parallel
// limitation note).
//
// Run: k6 run -e BASE_URL=https://staging.shovelsup.example \
//             -e SOURCE_DOCUMENT_IDS=uuid1,uuid2,uuid3,... \
//             loadtest/parse_load.js
import http from 'k6/http';
import { check } from 'k6';
import encoding from 'k6/encoding';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';
const ADMIN_USER = __ENV.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = __ENV.ADMIN_PASSWORD || '';
const SOURCE_DOCUMENT_IDS = (__ENV.SOURCE_DOCUMENT_IDS || '').split(',').filter(Boolean);

export const options = {
  scenarios: {
    mixed_format_batch: {
      executor: 'shared-iterations',
      vus: 10,
      iterations: 500,
      maxDuration: '5m',
    },
  },
  thresholds: {
    http_req_duration: ['p(95)<5000'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  if (SOURCE_DOCUMENT_IDS.length === 0) {
    throw new Error(
      'SOURCE_DOCUMENT_IDS env var is required (seed a mixed HTML/PDF/plain-text batch first)',
    );
  }

  const id = SOURCE_DOCUMENT_IDS[Math.floor(Math.random() * SOURCE_DOCUMENT_IDS.length)];
  const res = http.post(`${BASE_URL}/admin/source_documents/${id}/reprocess`, null, {
    headers: {
      Authorization: `Basic ${encoding.b64encode(`${ADMIN_USER}:${ADMIN_PASSWORD}`)}`,
    },
  });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'parser_status is a terminal state': (r) => {
      const body = JSON.parse(r.body);
      return ['parsed', 'failed', 'reprocessing'].includes(body.parser_status);
    },
  });
}
