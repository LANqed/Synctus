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
    fun `object commands serialise with only a type tag`() {
        val json = SynctusJson.encodeToString<BridgeCommand>(BridgeCommand.TogglePomodoro)
        assertEquals("{\"type\":\"toggle_pomodoro\"}", json)
    }

    @Test
    fun `peer event decodes from the rust shape`() {
        val json = """
            [{"type":"peer","name":"她的电脑","platform":"Windows","presence":"专注",
              "presence_color":4294198070,"detail":"♪ 歌手 - 歌名","meta":"🔋88%","stale":false}]
        """.trimIndent()

        val events = SynctusJson.decodeFromString<List<BridgeEvent>>(json)
        assertEquals(1, events.size)

        val peer = events.first() as BridgeEvent.Peer
        assertEquals("她的电脑", peer.name)
        assertEquals(4294198070L, peer.presenceColor)
        assertFalse(peer.stale)
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
        assertEquals(5, NudgeKey.all.size)
        NudgeKey.all.forEach { (kind, emoji, label) ->
            assertTrue(kind.isNotEmpty())
            assertTrue(emoji.isNotEmpty())
            assertTrue(label.isNotEmpty())
        }
    }
}
