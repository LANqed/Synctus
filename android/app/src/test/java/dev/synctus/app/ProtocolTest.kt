package dev.synctus.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlinx.serialization.encodeToString

/**
 * Guards the JSON contract with the Rust bridge.
 *
 * These names are the actual wire format; a rename on either side silently breaks
 * syncing, so they are pinned here rather than trusted to review.
 */
class ProtocolTest {

    @Test
    fun `publish command uses the field names rust expects`() {
        val command = BridgeCommand.Publish(
            battery = Battery(percent = 55, charging = true, minutesLeft = 90),
            foreground = ForegroundApp(app = "com.example", name = "Example"),
        )
        val json = SynctusJson.encodeToString<BridgeCommand>(command)

        assertTrue(json, json.contains("\"type\":\"publish\""))
        assertTrue(json, json.contains("\"percent\":55"))
        assertTrue(json, json.contains("\"minutes_left\":90"))
        assertTrue(json, json.contains("\"app\":\"com.example\""))
    }

    @Test
    fun `nudge command serialises the snake_case kind`() {
        val json = SynctusJson.encodeToString<BridgeCommand>(
            BridgeCommand.Nudge(NudgeKey.FOCUS_TOGETHER)
        )
        assertTrue(json, json.contains("\"type\":\"nudge\""))
        assertTrue(json, json.contains("\"kind\":\"focus_together\""))
    }

    @Test
    fun `nudge command can carry text`() {
        val json = SynctusJson.encodeToString<BridgeCommand>(
            BridgeCommand.Nudge(NudgeKey.NAG, "起来干活")
        )
        assertTrue(json, json.contains("\"kind\":\"nag\""))
        assertTrue(json, json.contains("\"text\":\"起来干活\""))
    }

    @Test
    fun `restore progress uses the field names rust expects`() {
        val json = SynctusJson.encodeToString<BridgeCommand>(
            BridgeCommand.RestoreProgress(focusTodayMin = 75, streakDays = 4)
        )
        assertTrue(json, json.contains("\"type\":\"restore_progress\""))
        assertTrue(json, json.contains("\"focus_today_min\":75"))
        assertTrue(json, json.contains("\"streak_days\":4"))
    }

    @Test
    fun `object commands serialise with only a type tag`() {
        val json = SynctusJson.encodeToString<BridgeCommand>(BridgeCommand.TogglePomodoro)
        assertEquals("{\"type\":\"toggle_pomodoro\"}", json)
    }

    @Test
    fun `peer event decodes from the rust shape`() {
        val json = """
            [{"type":"peer","name":"她的电脑","platform":"Windows","presence":"专注",
              "presence_color":4294198070,"detail":"♪ 歌手 - 歌名","meta":"🔋88%","stale":false,
              "focus_today_min":75,"goal_min":100,"streak_days":3,
              "focusing":true,"slacking":true}]
        """.trimIndent()

        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(json)
        assertEquals(1, events.size)

        val peer = events.first() as BridgeEvent.Peer
        assertEquals("她的电脑", peer.name)
        assertEquals(4294198070L, peer.presenceColor)
        assertFalse(peer.stale)
        assertEquals(75, peer.focusTodayMin)
        assertEquals(3, peer.streakDays)
        assertTrue(peer.focusing)
        assertTrue(peer.slacking)
        assertEquals(0.75f, peer.goalProgress())
        assertFalse(peer.goalMet())
    }

    @Test
    fun `peer accountability fields default when an older engine omits them`() {
        // Forward and backward compatibility: a peer running an older build sends
        // no focus numbers, and the UI must render rather than fail to decode.
        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"peer","name":"n","platform":"Android","presence":"在忙",
                 "presence_color":1,"detail":"d","meta":"","stale":false}]"""
        )
        val peer = events.first() as BridgeEvent.Peer
        assertEquals(0, peer.focusTodayMin)
        assertEquals(0, peer.goalMin)
        assertFalse(peer.focusing)
        assertFalse(peer.slacking)
        // No goal means no progress, rather than a division by zero.
        assertEquals(0f, peer.goalProgress())
    }

    @Test
    fun `goal reached event decodes`() {
        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"goal_reached","goal_min":100,"streak_days":5}]"""
        )
        val goal = events.first() as BridgeEvent.GoalReached
        assertEquals(100, goal.goalMin)
        assertEquals(5, goal.streakDays)
    }

    @Test
    fun `urgent nudges are flagged`() {
        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"nudge","title":"t","body":"b","kind":"Nag","urgent":true}]"""
        )
        val nudge = events.first() as BridgeEvent.Nudge
        assertTrue(nudge.urgent)

        // Absent means not urgent, so an older engine cannot accidentally
        // interrupt.
        val quiet = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"nudge","title":"t","body":"b","kind":"Knock"}]"""
        )
        assertFalse((quiet.first() as BridgeEvent.Nudge).urgent)
    }

    @Test
    fun `connection event tolerates a missing detail`() {
        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"connection","state":"online"}]"""
        )
        val connection = events.first() as BridgeEvent.Connection
        assertEquals("online", connection.state)
        assertEquals("", connection.detail)
    }

    @Test
    fun `unknown event fields do not break decoding`() {
        // Forward compatibility: a newer Rust build may add fields.
        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(
            """[{"type":"warning","message":"x","future_field":1}]"""
        )
        assertEquals("x", (events.first() as BridgeEvent.Warning).message)
    }

    @Test
    fun `empty event array decodes to an empty list`() {
        assertTrue(SynctusJson.decodeFromString<List<BridgeEvent>>("[]").isEmpty())
    }

    @Test
    fun `local status falls back to defaults for missing fields`() {
        val status = SynctusJson.decodeFromString<LocalStatus>("""{"presence":"休息中"}""")
        assertEquals("休息中", status.presence)
        assertFalse(status.pomodoroActive)
        assertEquals("00:00", status.pomodoroRemaining)
        assertEquals(0, status.focusTodayMin)
        assertFalse(status.goalMet)
        assertFalse(status.distracted)
    }

    @Test
    fun `local status carries the accountability numbers`() {
        val status = SynctusJson.decodeFromString<LocalStatus>(
            """{"focus_today_min":50,"goal_min":100,"streak_days":2,"goal_met":false,
                "peer_focus_today_min":75,"peer_focusing":true,
                "distracted":true,"distracted_by":"com.bilibili.app"}"""
        )
        assertEquals(50, status.focusTodayMin)
        assertEquals(75, status.peerFocusTodayMin)
        assertEquals(2, status.streakDays)
        assertTrue(status.peerFocusing)
        assertTrue(status.distracted)
        assertEquals("com.bilibili.app", status.distractedBy)
        assertEquals(0.5f, status.goalProgress())
        assertEquals(50, status.remainingMin())
    }

    @Test
    fun `goal progress is clamped and safe without a goal`() {
        val over = LocalStatus(focusTodayMin = 250, goalMin = 100)
        assertEquals(1f, over.goalProgress())
        assertEquals(0, over.remainingMin())

        val none = LocalStatus(focusTodayMin = 30, goalMin = 0)
        assertEquals(0f, none.goalProgress())
    }

    @Test
    fun `config round trips through json`() {
        val config = ClientConfig(
            server = "sync.example.com:8787",
            inviteCode = "ABCD-EFGH-IJKL-MNOP",
            deviceId = "abcd1234",
        )
        val json = SynctusJson.encodeToString(config)
        val back = SynctusJson.decodeFromString<ClientConfig>(json)

        assertEquals(config, back)
        assertTrue(json, json.contains("\"invite_code\""))
        assertTrue(json, json.contains("\"device_id\""))
        assertTrue(json, json.contains("\"poll_secs\""))
    }

    @Test
    fun `accountability config uses the field names rust expects`() {
        val json = SynctusJson.encodeToString(ClientConfig())
        assertTrue(json, json.contains("\"accountability\""))
        assertTrue(json, json.contains("\"daily_goal_min\""))
        assertTrue(json, json.contains("\"warn_on_distraction\""))
        assertTrue(json, json.contains("\"distracting_apps\""))
        assertTrue(json, json.contains("\"distraction_grace_secs\""))
        assertTrue(json, json.contains("\"report_distraction_to_peer\""))
        assertTrue(json, json.contains("\"allow_urgent_nudges\""))
        assertTrue(json, json.contains("\"auto_cheer\""))
    }

    @Test
    fun `accountability defaults match the rust side`() {
        // A mismatch here would silently change behaviour between the settings
        // screen and the engine.
        val acc = Accountability()
        assertEquals(100, acc.dailyGoalMin)
        assertTrue(acc.warnOnDistraction)
        assertEquals(30, acc.distractionGraceSecs)
        assertFalse("watching must be opt-in", acc.reportDistractionToPeer)
        assertTrue(acc.allowUrgentNudges)
        assertTrue(acc.autoCheer)
        assertTrue(acc.distractingApps.isNotEmpty())
    }

    @Test
    fun `pairing requires eight alphanumeric characters`() {
        assertFalse(ClientConfig(inviteCode = "").isPaired())
        assertFalse(ClientConfig(inviteCode = "AB-CD").isPaired())
        assertTrue(ClientConfig(inviteCode = "ABCD-EFGH").isPaired())
        assertTrue(ClientConfig(inviteCode = "abcdefgh").isPaired())
    }

    @Test
    fun `todo round trips with the rust field names`() {
        val todo = Todo(id = "a1", title = "写文档", createdAt = 1700000000000, pomodoros = 2)
        val json = SynctusJson.encodeToString(todo)
        assertTrue(json, json.contains("\"created_at\""))
        assertEquals(todo, SynctusJson.decodeFromString<Todo>(json))
    }

    @Test
    fun `presence keys match the rust serialisation`() {
        // These strings are deserialised by serde into the Presence enum.
        val expected = setOf("active", "resting", "away", "busy")
        assertEquals(expected, PresenceKey.selectable.map { it.first }.toSet())
    }

    @Test
    fun `every nudge kind has an emoji and a label`() {
        assertEquals(7, NudgeKey.all.size)
        NudgeKey.all.forEach { (kind, emoji, label) ->
            assertTrue(kind.isNotEmpty())
            assertTrue(emoji.isNotEmpty())
            assertTrue(label.isNotEmpty())
        }
    }

    @Test
    fun `the accountability nudges come first`() {
        // Order matters: a nag buried at the end of the row does not get used.
        assertEquals(NudgeKey.NAG, NudgeKey.all.first().first)
        assertEquals(NudgeKey.FOCUS_TOGETHER, NudgeKey.all[1].first)
    }

    @Test
    fun `nudge keys match the rust serialisation`() {
        val expected = setOf(
            "knock", "hug", "coffee", "rest", "focus_together", "nag", "cheer",
        )
        assertEquals(expected, NudgeKey.all.map { it.first }.toSet())
    }
}
