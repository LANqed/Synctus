"""今天也陪着你：让 Bot 拥有自己一天的日程，在日程切换与空闲时主动找用户说话。

日程模型与默认作息参考了 astrbot_plugin_private_companion（作者 menglimi）的
五段窗口划分与默认日程/作息锚点，简化为无需 LLM 的轻量实现。
"""

import asyncio
import json
import random
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Optional

from astrbot.api import AstrBotConfig, logger
from astrbot.api.event import AstrMessageEvent, MessageChain, filter
from astrbot.api.message_components import Plain
from astrbot.api.star import Context, Star, StarTools

PLUGIN_NAME = "astrbot_plugin_daily_companion"

SCHEDULE_WINDOWS = (
    ("深夜", 21 * 60, 6 * 60),
    ("早晨", 6 * 60, 11 * 60),
    ("中午", 11 * 60, 14 * 60 + 30),
    ("下午", 14 * 60 + 30, 18 * 60),
    ("晚上", 18 * 60, 21 * 60),
)

DEFAULT_SCHEDULE = [
    {
        "start": "07:30", "end": "08:10", "activity": "起床洗漱", "mood": "迷糊",
        "interruptible": True,
        "messages": [
            "早安～刚睁开眼，还有点迷糊……你昨晚睡得好吗",
            "唔……早安呀，刚爬起来，头发都翘了",
            "醒啦醒啦！新的一天开始咯，你起床了没",
        ],
    },
    {
        "start": "08:10", "end": "09:00", "activity": "吃早餐", "mood": "满足",
        "interruptible": True,
        "messages": [
            "早餐时间！干饭人干饭魂，不许饿肚子～",
            "在吃早餐啦，你也别空着肚子",
            "啃着早餐刷了会儿手机，忽然想到你",
        ],
    },
    {
        "start": "09:00", "end": "11:50", "activity": "专心做正事", "mood": "专注",
        "interruptible": False,
        "messages": [
            "上午的正事开始咯，我先去忙啦，回聊～",
            "要专心干活了！等我忙完再来找你",
            "上午认真工作中……想我了就随时喊我",
        ],
    },
    {
        "start": "11:50", "end": "13:30", "activity": "午餐和午休", "mood": "慵懒",
        "interruptible": True,
        "messages": [
            "午饭时间到！今天也要好好吃饭哦",
            "干饭人干饭魂！不许饿肚子～",
            "吃饱了有点犯困……我去眯一会儿，你也午休吗",
        ],
    },
    {
        "start": "13:30", "end": "16:30", "activity": "下午继续努力", "mood": "专注",
        "interruptible": False,
        "messages": [
            "下午也要加油呀，我继续去忙啦",
            "下午的活儿开始咯，忙完再来跟你分享今天的事",
            "午后继续努力中……你要也在忙，我们一起加油",
        ],
    },
    {
        "start": "16:30", "end": "17:30", "activity": "摸鱼休息", "mood": "放松",
        "interruptible": True,
        "messages": [
            "摸鱼时间！歇会儿啦，你也别太累咯",
            "忙完一大段了，现在瘫着休息……你那边怎么样呀",
            "下午茶休息中～突然想来看看你在干嘛",
        ],
    },
    {
        "start": "17:30", "end": "19:00", "activity": "晚餐时间", "mood": "开心",
        "interruptible": True,
        "messages": [
            "晚饭时间到！今天想吃点什么呀",
            "开饭啦开饭啦，一天里最幸福的时刻～",
            "在纠结晚饭吃什么……你有推荐的吗",
        ],
    },
    {
        "start": "19:00", "end": "21:00", "activity": "自由时间", "mood": "放松",
        "interruptible": True,
        "messages": [
            "晚上的自由时间开始咯，刷会儿视频放松一下～",
            "晚上闲下来啦，想跟你说说话",
            "一天里最舒服的时段～你在干嘛呀",
        ],
    },
    {
        "start": "21:00", "end": "22:30", "activity": "洗漱准备睡觉", "mood": "安静",
        "interruptible": True,
        "messages": [
            "要去洗漱准备睡觉啦，你今天过得怎么样",
            "快到睡觉时间了……今天还有没做完的事吗",
            "准备休息咯，今天辛苦啦",
        ],
    },
    {
        "start": "22:30", "end": "07:30", "activity": "睡觉", "mood": "熟睡",
        "interruptible": False,
        "messages": [
            "要去睡觉啦，晚安，好梦～",
            "困了困了……晚安呀，明天见",
            "晚安～今天也谢谢你陪着我，好梦",
        ],
    },
]

IDLE_TEMPLATES_WITH_ACTIVITY = [
    "刚刚在{activity}，突然想看看你在干嘛",
    "现在在{activity}，有点无聊，想找人说话……你在忙吗",
    "{activity}的间隙里想到你啦，随便聊聊？",
    "趁着{activity}的空隙休息一下下～你那边怎么样呀",
    "在{activity}的时候突然好奇：你今天过得还好吗",
]

IDLE_TEMPLATES_GENERIC = [
    "没什么事，就是想找你聊两句～",
    "发会儿呆……你要是在就好了",
    "刚想到一件事又忘了，算了，就想看看你在不在",
    "路过打个招呼，别嫌我烦呀",
    "突然想问问，你那边天气怎么样",
]

TICK_SECONDS = 60
TRANSITION_JITTER_RANGE = (10, 240)
TRANSITION_MIN_GAP_SECONDS = 300
DEFAULT_BOT_NAME = "小澈"
DEFAULT_MAX_DAILY = 15
DEFAULT_MIN_GAP_MINUTES = 40


def _parse_hhmm(value: Any) -> Optional[int]:
    try:
        parts = str(value).strip().split(":")
        if len(parts) != 2:
            return None
        hour, minute = int(parts[0]), int(parts[1])
        if not (0 <= hour <= 23 and 0 <= minute <= 59):
            return None
        return hour * 60 + minute
    except (ValueError, AttributeError, TypeError):
        return None


def _fmt_hhmm(minute: int) -> str:
    minute %= 1440
    return f"{minute // 60:02d}:{minute % 60:02d}"


def _span_contains(start: int, end: int, minute: int) -> bool:
    if start == end:
        return False
    if start < end:
        return start <= minute < end
    return minute >= start or minute < end


def _window_name(minute: int) -> str:
    for name, start, end in SCHEDULE_WINDOWS:
        if _span_contains(start, end, minute):
            return name
    return "白天"


def _parse_quiet(value: str) -> Optional[tuple]:
    value = (value or "").strip()
    if not value or value == "-":
        return None
    left, sep, right = value.partition("-")
    if not sep:
        return None
    start, end = _parse_hhmm(left), _parse_hhmm(right)
    if start is None or end is None or start == end:
        logger.warning(f"[DailyCompanion] 免打扰时段配置无效: {value}，已忽略")
        return None
    return (start, end)


def _copy_default_schedule() -> list:
    copied = []
    for item in DEFAULT_SCHEDULE:
        start = _parse_hhmm(item["start"])
        end = _parse_hhmm(item["end"])
        if start is None or end is None:
            continue
        copied.append(
            dict(
                item,
                start=start,
                end=end,
                messages=list(item["messages"]),
                full_day=False,
            )
        )
    return copied


def _build_schedule(config: AstrBotConfig) -> list:
    segments = []
    for entry in (config.get("custom_schedule") or []):
        if not isinstance(entry, dict):
            continue
        start = _parse_hhmm(entry.get("start"))
        end = _parse_hhmm(entry.get("end"))
        activity = str(entry.get("activity") or "").strip()
        if start is None or end is None or start == end or not activity:
            logger.warning(f"[DailyCompanion] 忽略无效日程段: {entry}")
            continue
        messages = [
            line.strip()
            for line in str(entry.get("messages") or "").splitlines()
            if line.strip()
        ]
        segments.append(
            {
                "start": start,
                "end": end,
                "activity": activity,
                "mood": str(entry.get("mood") or "平静").strip() or "平静",
                "interruptible": bool(entry.get("interruptible", True)),
                "messages": messages,
            }
        )
    if not segments:
        segments = _copy_default_schedule()
    segments.sort(key=lambda item: item["start"])
    if len(segments) == 1:
        segments[0]["end"] = segments[0]["start"]
        segments[0]["full_day"] = True
        return segments
    for i, segment in enumerate(segments):
        nxt = segments[(i + 1) % len(segments)]
        if segment["end"] != nxt["start"]:
            segment["end"] = nxt["start"]
    for segment in segments:
        segment.setdefault("full_day", False)
    return segments


def _default_user_state() -> dict:
    return {
        "muted": False,
        "date": "",
        "sent_today": 0,
        "last_sent_ts": 0.0,
        "last_segment": None,
        "pending_at": None,
        "pending_msg": None,
    }


def _load_states(path: Path) -> dict:
    try:
        if path.exists():
            data = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                return data
    except Exception as exc:
        logger.warning(f"[DailyCompanion] 读取状态文件失败，将重建: {exc}")
    return {}


def _save_states(path: Path, states: dict) -> None:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(states, ensure_ascii=False, indent=2), encoding="utf-8"
        )
    except Exception as exc:
        logger.warning(f"[DailyCompanion] 保存状态文件失败: {exc}")


class DailyCompanionPlugin(Star):
    def __init__(self, context: Context, config: AstrBotConfig):
        super().__init__(context)
        self.config = config
        self._segments = _build_schedule(config)
        self._quiet = _parse_quiet(str(config.get("quiet_hours") or ""))
        self._state_path = Path(StarTools.get_data_dir(PLUGIN_NAME)) / "state.json"
        self._user_states = _load_states(self._state_path)
        self._stop_event = asyncio.Event()
        self._task: Optional[asyncio.Task] = None

    async def initialize(self):
        targets = self._target_umos()
        if targets:
            logger.info(
                f"[DailyCompanion] 陪伴循环启动，目标用户: {', '.join(targets)}"
            )
        else:
            logger.warning(
                "[DailyCompanion] 未配置 target_users，插件只响应命令，不会主动发消息"
            )
        if self._task is None or self._task.done():
            self._task = asyncio.create_task(self._loop())

    async def terminate(self):
        self._stop_event.set()
        task, self._task = self._task, None
        if task is not None and not task.done():
            task.cancel()
            try:
                await asyncio.wait({task}, timeout=3)
            except Exception:
                pass
        _save_states(self._state_path, self._user_states)

    def _target_umos(self) -> list:
        seen, result = set(), []
        for entry in (self.config.get("target_users") or []):
            raw = str(entry).strip()
            if not raw:
                continue
            umo = raw if ":" in raw else f"aiocqhttp:FriendMessage:{raw}"
            if umo not in seen:
                seen.add(umo)
                result.append(umo)
        return result

    def _current_segment_index(self, minute: int) -> Optional[int]:
        if len(self._segments) == 1:
            return 0
        for index, segment in enumerate(self._segments):
            if _span_contains(segment["start"], segment["end"], minute):
                return index
        return None

    def _user_state(self, umo: str) -> dict:
        state = self._user_states.get(umo)
        if not isinstance(state, dict):
            state = _default_user_state()
            self._user_states[umo] = state
        today = datetime.now().strftime("%Y-%m-%d")
        if state.get("date") != today:
            state["date"] = today
            state["sent_today"] = 0
        return state

    def _in_quiet(self, now: datetime) -> bool:
        if not self._quiet:
            return False
        minute = now.hour * 60 + now.minute
        return _span_contains(self._quiet[0], self._quiet[1], minute)

    def _gate(self, state: dict, idle: bool, now: datetime) -> bool:
        if state.get("muted"):
            return False
        max_daily = int(self.config.get("max_daily_messages") or DEFAULT_MAX_DAILY)
        if int(state.get("sent_today") or 0) >= max_daily:
            return False
        last = float(state.get("last_sent_ts") or 0)
        if last:
            if idle:
                gap = int(
                    self.config.get("min_gap_minutes") or DEFAULT_MIN_GAP_MINUTES
                ) * 60
            else:
                gap = TRANSITION_MIN_GAP_SECONDS
            if time.time() - last < gap:
                return False
        if idle and self._in_quiet(now):
            return False
        return True

    async def _send(self, umo: str, text: str) -> bool:
        try:
            await self.context.send_message(umo, MessageChain([Plain(text)]))
        except Exception as exc:
            logger.error(f"[DailyCompanion] 主动消息发送失败 ({umo}): {exc}")
            return False
        state = self._user_state(umo)
        state["sent_today"] = int(state.get("sent_today") or 0) + 1
        state["last_sent_ts"] = time.time()
        logger.info(f"[DailyCompanion] 已发送主动消息到 {umo}: {text[:40]}")
        return True

    def _compose_idle(self, segment: dict) -> str:
        if random.random() < 0.65:
            return random.choice(IDLE_TEMPLATES_WITH_ACTIVITY).format(
                activity=segment["activity"]
            )
        return random.choice(IDLE_TEMPLATES_GENERIC)

    async def _loop(self):
        logger.info("[DailyCompanion] 日程陪伴循环运行中")
        while not self._stop_event.is_set():
            try:
                await asyncio.wait_for(
                    self._stop_event.wait(), timeout=TICK_SECONDS
                )
            except asyncio.TimeoutError:
                pass
            else:
                break
            try:
                await self._tick()
            except Exception:
                logger.exception("[DailyCompanion] tick 执行异常")
        logger.info("[DailyCompanion] 日程陪伴循环已退出")

    async def _tick(self):
        now = datetime.now()
        targets = self._target_umos()
        if not targets:
            return
        minute = now.hour * 60 + now.minute
        index = self._current_segment_index(minute)
        if index is None:
            return
        segment = self._segments[index]
        dirty = False
        for umo in targets:
            state = self._user_state(umo)
            pending_at = state.get("pending_at")
            if pending_at is not None and time.time() >= float(pending_at):
                message = state.get("pending_msg")
                state["pending_at"] = None
                state["pending_msg"] = None
                dirty = True
                if message and self._gate(state, idle=False, now=now):
                    if await self._send(umo, message):
                        dirty = True
            if state.get("last_segment") != index:
                first_observation = state.get("last_segment") is None
                state["last_segment"] = index
                dirty = True
                if (
                    not first_observation
                    and pending_at is None
                    and self.config.get("enable_transition", True)
                    and segment["messages"]
                ):
                    if self._gate(state, idle=False, now=now):
                        state["pending_at"] = time.time() + random.uniform(
                            *TRANSITION_JITTER_RANGE
                        )
                        state["pending_msg"] = random.choice(segment["messages"])
                        dirty = True
            if (
                self.config.get("enable_idle_chat", True)
                and state.get("pending_at") is None
                and segment["interruptible"]
            ):
                chance = float(self.config.get("idle_chance_per_minute") or 0.0)
                if chance > 0 and random.random() < chance:
                    if self._gate(state, idle=True, now=now):
                        if await self._send(umo, self._compose_idle(segment)):
                            dirty = True
        if dirty:
            _save_states(self._state_path, self._user_states)

    def _render_status(self, umo: str, now: datetime) -> str:
        bot_name = str(self.config.get("bot_name") or "").strip() or DEFAULT_BOT_NAME
        minute = now.hour * 60 + now.minute
        index = self._current_segment_index(minute)
        if index is None:
            return f"{bot_name}的日程表好像有点乱，让管理员看看配置吧"
        segment = self._segments[index]
        elapsed = (minute - segment["start"]) % 1440
        lines = [
            f"{bot_name}现在：{segment['activity']}（{segment['mood']}）"
            f"· {now.strftime('%H:%M')} · {_window_name(minute)}",
            f"这一段 {_fmt_hhmm(segment['start'])}-{_fmt_hhmm(segment['end'])}，"
            f"已经 {elapsed} 分钟",
        ]
        if len(self._segments) > 1:
            nxt = self._segments[(index + 1) % len(self._segments)]
            eta = (nxt["start"] - minute) % 1440
            lines.append(
                f"下一段：{nxt['activity']}（{_fmt_hhmm(nxt['start'])}，还有 {eta} 分钟）"
            )
        if umo:
            state = self._user_state(umo)
            max_daily = int(
                self.config.get("max_daily_messages") or DEFAULT_MAX_DAILY
            )
            lines.append(
                f"今天已经主动找你 {int(state.get('sent_today') or 0)} 次"
                f"（上限 {max_daily}）"
            )
            if state.get("muted"):
                lines.append("你现在处于静音状态，我不会主动打扰")
        if self._in_quiet(now):
            lines.append("现在是免打扰时段，你找我的时候我才会说话")
        return "\n".join(lines)

    def _render_schedule(self, now: datetime) -> str:
        bot_name = str(self.config.get("bot_name") or "").strip() or DEFAULT_BOT_NAME
        minute = now.hour * 60 + now.minute
        current = self._current_segment_index(minute)
        lines = [f"{bot_name}的一天："]
        for index, segment in enumerate(self._segments):
            mark = "  <- 现在" if index == current else ""
            lines.append(
                f"{_fmt_hhmm(segment['start'])}-{_fmt_hhmm(segment['end'])} "
                f"{segment['activity']}（{segment['mood']}）{mark}"
            )
        if self._quiet:
            lines.append(
                f"免打扰 {_fmt_hhmm(self._quiet[0])}-{_fmt_hhmm(self._quiet[1])}"
                "（期间不随机搭话）"
            )
        return "\n".join(lines)

    def _handle_sub_command(self, sub: str, umo: str) -> str:
        now = datetime.now()
        if sub in {"静音", "闭嘴", "安静"}:
            if umo:
                state = self._user_state(umo)
                state["muted"] = True
                _save_states(self._state_path, self._user_states)
            return "好……我会安静陪着你，不主动打扰啦。想我了就发「陪伴 恢复」"
        if sub in {"恢复", "开口"}:
            if umo:
                state = self._user_state(umo)
                state["muted"] = False
                _save_states(self._state_path, self._user_states)
            return "我又回来啦～刚刚有没有想我？"
        if sub in {"日程", "今日", "今天", "安排"}:
            return self._render_schedule(now)
        if sub in {"帮助", "help", "命令"}:
            return (
                "陪伴 状态 - 看我现在在做什么\n"
                "陪伴 日程 - 看我今天的安排\n"
                "陪伴 静音 / 陪伴 恢复 - 暂停或恢复主动消息\n"
                "日程 - 等价于「陪伴 日程」\n"
                "我会在日程切换（早安、去专注、午休、晚安）和空闲时主动找你说话"
            )
        return self._render_status(umo, now)

    @filter.command("陪伴", alias={"日程陪伴", "每日陪伴"})
    async def companion_command(self, event: AstrMessageEvent):
        """查看 Bot 当前日程与陪伴状态；支持 状态/日程/静音/恢复/帮助 子命令。"""
        text = (
            str(getattr(event, "message_str", "") or "")
            .replace("／", "/")
            .replace("\u3000", " ")
            .strip()
        )
        parts = text.split()
        sub = parts[1] if len(parts) > 1 else "状态"
        umo = str(getattr(event, "unified_msg_origin", "") or "")
        yield event.plain_result(self._handle_sub_command(sub, umo))

    @filter.command("日程", alias={"bot日程", "今日日程"})
    async def schedule_command(self, event: AstrMessageEvent):
        """查看 Bot 今天的日程安排。"""
        umo = str(getattr(event, "unified_msg_origin", "") or "")
        yield event.plain_result(self._render_schedule(datetime.now()))
