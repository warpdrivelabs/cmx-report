#!/usr/bin/env bash
# C7 范围变动 E2E:两期范围对比 → 四类变动分类 + 幂等。
# 前置:report-server :8092 在跑,CAS_LEGAL(2026-06/2026-12)+ SC_TEST(2026-01/2026-02)已 seed。
set -u
B=http://localhost:8092/api/consol
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1"; fail=$((fail+1)); }

echo "=== 1. CAS_LEGAL 2026-12 vs 2026-06(自动取上期) ==="
r=$(curl -s -m 10 -H 'content-type: application/json' -X POST $B/scope-change -d '{"scheme":"CAS_LEGAL","period":"2026-12"}')
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
c=d["counts"]
print(("PASS" if c.get("disposal")==3 else "FAIL")+"|CAS 处置3项(HQ/S1/S2)="+str(c.get("disposal")))
print(("PASS" if c.get("first_time")==4 else "FAIL")+"|CAS 新纳入4项(DIRECT/SG/SG_A/SG_B)="+str(c.get("first_time")))
print(("PASS" if d["prev"]=="2026-06" else "FAIL")+"|CAS 自动上期=2026-06 got="+d["prev"])
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg"; done

echo "=== 2. SC_TEST 四类变动齐全 ==="
curl -s -m 10 -H 'content-type: application/json' -X POST $B/scope-change -d '{"scheme":"SC_TEST","period":"2026-02","prev_period":"2026-01"}' >/dev/null
r=$(curl -s -m 8 "$B/scope-change?scheme=SC_TEST&period=2026-02")
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
m={x["org_code"]:x["change_type"] for x in d["rows"]}
for org,ct in [("A","ownership_up"),("B","method_change"),("C","disposal"),("D","first_time")]:
    print(("PASS" if m.get(org)==ct else "FAIL")+f"|SC {org}={m.get(org)} exp={ct}")
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg"; done

echo "=== 3. 幂等(重跑不产生重复) ==="
curl -s -m 8 -H 'content-type: application/json' -X POST $B/scope-change -d '{"scheme":"SC_TEST","period":"2026-02","prev_period":"2026-01"}' >/dev/null
n=$(curl -s -m 8 "$B/scope-change?scheme=SC_TEST&period=2026-02" | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["count"])')
[ "$n" = "4" ] && ok "重跑后仍 4 项(幂等)" || no "幂等 count=$n"

echo ""
echo "=== $pass passed / $fail failed ==="
[ "$fail" = "0" ] && exit 0 || exit 1
