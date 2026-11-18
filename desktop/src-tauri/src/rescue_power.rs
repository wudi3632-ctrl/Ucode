use crate::rescue::{EvidenceNode, EvidenceState, Hypothesis, HypothesisState, ProbeOutput};
use serde_json::Value;

const TOOL_ID: &str = "system.power.inspect";
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

pub fn is_power_request(request: &str) -> bool {
    let value = request.to_lowercase();
    let explanatory = [
        "什么是",
        "是什么意思",
        "怎么保护",
        "如何保护",
        "what is",
        "explain",
        "介绍",
    ]
    .iter()
    .any(|marker| value.contains(marker));
    if explanatory {
        return false;
    }
    [
        "电池电量低",
        "电池快没电",
        "电池没电",
        "电池不充电",
        "插电不充电",
        "插电但没充电",
        "插电没反应",
        "充电异常",
        "电源故障",
        "充电状态",
        "电源状态",
        "突然断电",
        "拔掉电源就关机",
        "拔电就关机",
        "电池耗电快",
        "电池掉电快",
        "battery low",
        "low battery",
        "battery not charging",
        "plugged in not charging",
        "power status",
        "sudden shutdown",
        "shuts down when unplugged",
        "battery draining fast",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

pub fn requested_probe(request: &str) -> Option<(String, Value)> {
    is_power_request(request).then(|| (TOOL_ID.into(), Value::Null))
}

pub fn run(tool_id: &str, arguments: &Value) -> Option<Result<ProbeOutput, String>> {
    (tool_id == TOOL_ID).then(|| run_power(arguments))
}

fn run_power(arguments: &Value) -> Result<ProbeOutput, String> {
    if !(arguments.is_null() || arguments.as_object().is_some_and(|value| value.is_empty())) {
        return Err("电源探针不接受模型参数".into());
    }
    let structured = power_native::query()?;
    let stdout =
        serde_json::to_vec(&structured).map_err(|error| format!("无法序列化电源证据：{error}"))?;
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err("电源证据超出有界输出限制".into());
    }
    Ok(ProbeOutput {
        exit_code: 0,
        stdout: String::from_utf8(stdout)
            .map_err(|error| format!("电源证据不是有效 UTF-8：{error}"))?,
        stderr: String::new(),
    })
}

#[cfg(target_os = "windows")]
mod power_native {
    use serde_json::{json, Value};

    const BATTERY_FLAG_HIGH: u8 = 0x01;
    const BATTERY_FLAG_LOW: u8 = 0x02;
    const BATTERY_FLAG_CRITICAL: u8 = 0x04;
    const BATTERY_FLAG_CHARGING: u8 = 0x08;
    const BATTERY_FLAG_NO_BATTERY: u8 = 0x80;
    const BATTERY_FLAG_UNKNOWN: u8 = 0xff;
    const KNOWN_BATTERY_FLAGS: u8 = BATTERY_FLAG_HIGH
        | BATTERY_FLAG_LOW
        | BATTERY_FLAG_CRITICAL
        | BATTERY_FLAG_CHARGING
        | BATTERY_FLAG_NO_BATTERY;
    const UNKNOWN_LIFETIME: u32 = u32::MAX;

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn query() -> Result<Value, String> {
        let mut status: SystemPowerStatus = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return Err(format!("Windows 无法读取电源状态：错误 {}", unsafe {
                GetLastError()
            }));
        }
        normalize(status)
    }

    fn normalize(status: SystemPowerStatus) -> Result<Value, String> {
        let ac_line_status = match status.ac_line_status {
            0 => "offline",
            1 => "online",
            255 => "unknown",
            _ => return Err("Windows 返回了无效的交流电状态".into()),
        };
        if status.system_status_flag > 1 {
            return Err("Windows 返回了无效的节电模式状态".into());
        }
        if status.battery_life_percent > 100 && status.battery_life_percent != 255 {
            return Err("Windows 返回了无效的电池百分比".into());
        }

        let flag_unknown = status.battery_flag == BATTERY_FLAG_UNKNOWN;
        let no_battery = !flag_unknown && status.battery_flag & BATTERY_FLAG_NO_BATTERY != 0;
        let battery_present = (!flag_unknown).then_some(!no_battery);
        let charging = (!flag_unknown && !no_battery)
            .then_some(status.battery_flag & BATTERY_FLAG_CHARGING != 0);
        let critical = (!flag_unknown && !no_battery)
            .then_some(status.battery_flag & BATTERY_FLAG_CRITICAL != 0);
        let battery_percent = (!flag_unknown && !no_battery && status.battery_life_percent != 255)
            .then_some(status.battery_life_percent);
        let lifetime_unknown = status.battery_life_time == UNKNOWN_LIFETIME;
        let estimated_remaining_seconds =
            (!lifetime_unknown && !no_battery && !flag_unknown).then_some(status.battery_life_time);
        let unknown_bits = flag_unknown || status.battery_flag & !KNOWN_BATTERY_FLAGS != 0;

        Ok(json!({
            "acLineStatus": ac_line_status,
            "batteryPresent": battery_present,
            "charging": charging,
            "critical": critical,
            "batterySaver": status.system_status_flag == 1,
            "batteryPercent": battery_percent,
            "estimatedRemainingSeconds": estimated_remaining_seconds,
            "lifetimeUnknown": lifetime_unknown,
            "rawBatteryFlagUnknownBits": unknown_bits,
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn status(ac: u8, flags: u8, percent: u8, saver: u8, lifetime: u32) -> SystemPowerStatus {
            SystemPowerStatus {
                ac_line_status: ac,
                battery_flag: flags,
                battery_life_percent: percent,
                system_status_flag: saver,
                battery_life_time: lifetime,
                battery_full_life_time: UNKNOWN_LIFETIME,
            }
        }

        #[test]
        fn normalization_distinguishes_charging_no_battery_and_unknown_values() {
            let charging = normalize(status(1, BATTERY_FLAG_CHARGING, 76, 0, 3_600)).unwrap();
            assert_eq!(charging["acLineStatus"], "online");
            assert_eq!(charging["batteryPresent"], true);
            assert_eq!(charging["charging"], true);
            assert_eq!(charging["batteryPercent"], 76);
            assert_eq!(charging["estimatedRemainingSeconds"], 3_600);

            let desktop =
                normalize(status(1, BATTERY_FLAG_NO_BATTERY, 255, 0, UNKNOWN_LIFETIME)).unwrap();
            assert_eq!(desktop["batteryPresent"], false);
            assert!(desktop["batteryPercent"].is_null());
            assert!(desktop["charging"].is_null());

            let unknown = normalize(status(255, BATTERY_FLAG_UNKNOWN, 255, 1, 1_800)).unwrap();
            assert_eq!(unknown["acLineStatus"], "unknown");
            assert!(unknown["batteryPresent"].is_null());
            assert!(unknown["estimatedRemainingSeconds"].is_null());
            assert_eq!(unknown["batterySaver"], true);
            assert_eq!(unknown["rawBatteryFlagUnknownBits"], true);
        }

        #[test]
        fn normalization_rejects_invalid_native_values() {
            assert!(normalize(status(2, 0, 50, 0, 1)).is_err());
            assert!(normalize(status(1, 0, 101, 0, 1)).is_err());
            assert!(normalize(status(1, 0, 50, 2, 1)).is_err());
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod power_native {
    use serde_json::Value;

    pub fn query() -> Result<Value, String> {
        Err("电源探针只支持 Windows".into())
    }
}

pub fn analyze(evidence: &[EvidenceNode]) -> Option<(Vec<Hypothesis>, Option<String>)> {
    let item = evidence.iter().find(|item| item.tool_id == TOOL_ID)?;
    if !matches!(item.state, EvidenceState::Observed) {
        return Some((
            vec![Hypothesis {
                id: "system.power_status_unknown".into(),
                title: "无法确认当前电源状态".into(),
                state: HypothesisState::Unresolved,
                confidence: 0.2,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("电源探针不可用；未把失败误报为供电正常，也没有修改电源计划".into()),
        ));
    }

    let ac = item.structured.get("acLineStatus").and_then(Value::as_str);
    let present = item
        .structured
        .get("batteryPresent")
        .and_then(Value::as_bool);
    let charging = item.structured.get("charging").and_then(Value::as_bool);
    let critical = item.structured.get("critical").and_then(Value::as_bool);
    let percent = item
        .structured
        .get("batteryPercent")
        .and_then(Value::as_u64)
        .filter(|value| *value <= 100);

    let Some(ac) = ac.filter(|value| matches!(*value, "online" | "offline" | "unknown")) else {
        return Some(incomplete(item));
    };
    let Some(present) = present else {
        return Some(incomplete(item));
    };
    if !present {
        return Some((
            vec![Hypothesis {
                id: "system.battery_presence".into(),
                title: "Windows 当前未检测到系统电池".into(),
                state: HypothesisState::Contradicted,
                confidence: 0.99,
                evidence_ids: vec![item.id.clone()],
            }],
            Some("当前设备未报告电池；若这是一台应有电池的设备，需要检查固件或硬件识别。本轮没有修改电源计划".into()),
        ));
    }

    let Some(charging) = charging else {
        return Some(incomplete(item));
    };
    let Some(critical) = critical else {
        return Some(incomplete(item));
    };
    let percent_text = percent
        .map(|value| format!("，电量 {value}%"))
        .unwrap_or_default();
    let (title, state, confidence, next) =
        if critical || (ac == "offline" && percent.is_some_and(|value| value <= 10)) {
            (
                format!("当前电池处于临界低电量{percent_text}"),
                HypothesisState::Supported,
                0.99,
                "请尽快连接可靠电源；UCode 本轮没有修改电源计划或设备状态",
            )
        } else if ac == "offline" && percent.is_some_and(|value| value <= 20) {
            (
                format!("当前电池电量偏低{percent_text}"),
                HypothesisState::Supported,
                0.98,
                "建议连接电源；单次快照不能判断电池健康度，UCode 没有修改任何设置",
            )
        } else if ac == "online" && charging {
            (
                format!("当前已接通交流电且 Windows 报告正在充电{percent_text}"),
                HypothesisState::Contradicted,
                0.99,
                "当前快照反驳“完全未充电”，但不能证明充电速度、适配器功率或电池健康度",
            )
        } else if ac == "online" {
            (
                format!("当前已接通交流电，但 Windows 未报告正在充电{percent_text}"),
                HypothesisState::Unresolved,
                0.7,
                "充电阈值或电池保护模式也可能暂停充电；仅凭当前快照不能判定充电器、电池或主板故障",
            )
        } else if ac == "offline" {
            (
                format!("当前使用电池供电，未达到低电量阈值{percent_text}"),
                HypothesisState::Contradicted,
                0.95,
                "当前快照没有显示低电量；若存在间歇掉电，需要在复现时重新采样",
            )
        } else {
            (
                format!("Windows 无法确认当前交流电连接状态{percent_text}"),
                HypothesisState::Unresolved,
                0.3,
                "当前证据不足以判断供电来源；UCode 没有修改电源计划或设备状态",
            )
        };

    Some((
        vec![Hypothesis {
            id: "system.power_status".into(),
            title,
            state,
            confidence,
            evidence_ids: vec![item.id.clone()],
        }],
        Some(next.into()),
    ))
}

fn incomplete(item: &EvidenceNode) -> (Vec<Hypothesis>, Option<String>) {
    (
        vec![Hypothesis {
            id: "system.power_status_unknown".into(),
            title: "电源证据不完整".into(),
            state: HypothesisState::Unresolved,
            confidence: 0.2,
            evidence_ids: vec![item.id.clone()],
        }],
        Some("没有足够的电源状态证据；未显示伪造的电量，也没有修改电源计划".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn evidence(structured: Value, state: EvidenceState) -> EvidenceNode {
        let bytes = serde_json::to_vec(&structured).unwrap();
        EvidenceNode {
            id: "evidence-power".into(),
            tool_id: TOOL_ID.into(),
            tool_version: 1,
            state,
            summary: "power".into(),
            structured,
            exit_code: Some(0),
            content_digest: format!("{:x}", Sha256::digest(bytes)),
            elapsed_ms: 1,
        }
    }

    fn snapshot(
        ac: &str,
        present: Option<bool>,
        charging: Option<bool>,
        critical: Option<bool>,
        percent: Option<u8>,
    ) -> Value {
        json!({
            "acLineStatus":ac,
            "batteryPresent":present,
            "charging":charging,
            "critical":critical,
            "batterySaver":false,
            "batteryPercent":percent,
            "estimatedRemainingSeconds":null,
            "lifetimeUnknown":true,
            "rawBatteryFlagUnknownBits":false
        })
    }

    #[test]
    fn parser_accepts_explicit_power_faults_and_rejects_explanations_or_generic_slowness() {
        for (request, expected) in [
            ("电池快没电了，帮我检查", true),
            ("插电但没充电", true),
            ("电脑突然断电", true),
            ("充电异常，请检查", true),
            ("battery not charging", true),
            ("battery draining fast", true),
            ("什么是节电模式", false),
            ("what is battery saver", false),
            ("电脑很慢", false),
        ] {
            assert_eq!(is_power_request(request), expected, "{request}");
        }
    }

    #[test]
    fn analysis_distinguishes_low_charging_not_charging_no_battery_and_unavailable() {
        let cases = [
            (
                snapshot("offline", Some(true), Some(false), Some(true), Some(8)),
                HypothesisState::Supported,
            ),
            (
                snapshot("offline", Some(true), Some(false), Some(false), Some(18)),
                HypothesisState::Supported,
            ),
            (
                snapshot("online", Some(true), Some(true), Some(false), Some(76)),
                HypothesisState::Contradicted,
            ),
            (
                snapshot("online", Some(true), Some(false), Some(false), Some(80)),
                HypothesisState::Unresolved,
            ),
            (
                snapshot("online", Some(false), None, None, None),
                HypothesisState::Contradicted,
            ),
        ];
        for (structured, expected) in cases {
            let (hypotheses, _) =
                analyze(&[evidence(structured, EvidenceState::Observed)]).unwrap();
            assert_eq!(
                std::mem::discriminant(&hypotheses[0].state),
                std::mem::discriminant(&expected)
            );
        }
        let (hypotheses, next) = analyze(&[evidence(
            json!({"error":"denied"}),
            EvidenceState::Unavailable,
        )])
        .unwrap();
        assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        assert!(next.unwrap().contains("未把失败误报"));
    }

    #[test]
    fn invalid_or_unknown_power_fields_are_never_reported_as_healthy() {
        for value in [
            snapshot("invalid", Some(true), Some(false), Some(false), Some(50)),
            snapshot("online", None, None, None, None),
            snapshot("online", Some(true), None, Some(false), Some(50)),
        ] {
            let (hypotheses, _) = analyze(&[evidence(value, EvidenceState::Observed)]).unwrap();
            assert!(matches!(hypotheses[0].state, HypothesisState::Unresolved));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_probe_returns_bounded_private_power_metadata() {
        let output = run_power(&Value::Null).unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.len() <= MAX_OUTPUT_BYTES);
        let value: Value = serde_json::from_str(&output.stdout).unwrap();
        assert!(matches!(
            value["acLineStatus"].as_str(),
            Some("online" | "offline" | "unknown")
        ));
        if let Some(percent) = value["batteryPercent"].as_u64() {
            assert!(percent <= 100);
        }
        let raw = value.to_string();
        for private in [
            "serialNumber",
            "deviceInstance",
            "manufacturer",
            "model",
            "username",
            "powerPlanGuid",
        ] {
            assert!(!raw.contains(private), "private field leaked: {private}");
        }
    }
}
