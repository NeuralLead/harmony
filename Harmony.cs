using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.Json.Serialization.Metadata;

namespace OpenAI.Harmony
{
    // Helper: Lowercase enums for compatibility with Python and Rust conventions
    public class LowercaseEnumConverter<T> : JsonConverter<T> where T : Enum
    {
        public override T Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        {
            var enumString = reader.GetString();
            foreach (T enumValue in Enum.GetValues(typeof(T)))
            {
                if (enumValue.ToString().Equals(enumString, StringComparison.OrdinalIgnoreCase))
                    return enumValue;
            }
            throw new JsonException($"Unable to convert \"{enumString}\" to enum {typeof(T)}");
        }

        public override void Write(Utf8JsonWriter writer, T value, JsonSerializerOptions options)
        {
            writer.WriteStringValue(value.ToString().ToLower());
        }
    }

    // ---------------------------------------------------------------------------
    // Data Models: Chat-related data structures (mirroring `src/chat.rs`)
    // ---------------------------------------------------------------------------

    /// <summary>
    /// The role of a message author (mirrors `chat::Role`).
    /// </summary>
    public enum Role { User, Assistant, System, Developer, Tool }

    public class Author
    {
        [JsonPropertyName("role")]
        [JsonConverter(typeof(LowercaseEnumConverter<Role>))]
        public Role Role { get; set; } = new();
        [JsonPropertyName("name")]
        public string? Name { get; set; }

        public Author(Role role, string? name = null)
        {
            Role = role;
            Name = name;
        }
    }

    // Content hierarchy ---------------------------------------------------------

    /// <summary>
    /// Abstract class for message content as in Python's Content(BaseModel).
    /// </summary>
    [JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
    [JsonDerivedType(typeof(TextContent), "text")]
    [JsonDerivedType(typeof(SystemContent), "system_content")]
    [JsonDerivedType(typeof(DeveloperContent), "developer_content")]
    public abstract class Content
    {
        public abstract Dictionary<string, object?> ToDictionary();
    }

    public class TextContent : Content
    {
        [JsonPropertyName("text")]
        public string Text { get; set; }
        public TextContent() { }
        public TextContent(string text) { Text = text; }
        public override Dictionary<string, object?> ToDictionary() => new() { { "type", "text" }, { "text", Text } };
        public override string ToString() => Text;
    }

    public class SystemContent : Content
    {
        // See Python SystemContent, mirrors public API and adds defaults and fluent methods

        [JsonPropertyName("model_identity")]
        public string? ModelIdentity { get; set; } = "You are ChatGPT, a large language model trained by OpenAI.";
        [JsonPropertyName("reasoning_effort")]
        [JsonConverter(typeof(JsonStringEnumConverter))]
        public ReasoningEffort? ReasoningEffort { get; set; } = Harmony.ReasoningEffort.Medium;
        [JsonPropertyName("conversation_start_date")]
        public string? ConversationStartDate { get; set; }
        [JsonPropertyName("knowledge_cutoff")]
        public string? KnowledgeCutoff { get; set; } = "2024-06";
        [JsonPropertyName("channel_config")]
        public ChannelConfig? ChannelConfig { get; set; } = ChannelConfig.RequireChannels(new() { "analysis", "commentary", "final" });
        [JsonPropertyName("tools")]
        public Dictionary<string, ToolNamespaceConfig>? Tools { get; set; }

        public static SystemContent New() => new();
        public SystemContent WithModelIdentity(string v) { ModelIdentity = v; return this; }
        public SystemContent WithReasoningEffort(ReasoningEffort val) { ReasoningEffort = val; return this; }
        public SystemContent WithTools(ToolNamespaceConfig nsCfg)
        {
            Tools ??= new();
            Tools[nsCfg.Name] = nsCfg;
            return this;
        }
        public override Dictionary<string, object?> ToDictionary()
        {
            var dict = new Dictionary<string, object?>
            {
                { "type", "system_content" },
                { "model_identity", ModelIdentity },
                { "reasoning_effort", ReasoningEffort?.ToString() },
                { "conversation_start_date", ConversationStartDate },
                { "knowledge_cutoff", KnowledgeCutoff },
                { "channel_config", ChannelConfig },
                { "tools", Tools }
            };
            return dict.Where(kv => kv.Value != null).ToDictionary(kv => kv.Key, kv => kv.Value);
        }
    }

    public class DeveloperContent : Content
    {
        [JsonPropertyName("instructions")]
        public string? Instructions { get; set; }
        [JsonPropertyName("tools")]
        public Dictionary<string, ToolNamespaceConfig>? Tools { get; set; }
        public static DeveloperContent New() => new();
        public DeveloperContent WithInstructions(string i) { Instructions = i; return this; }
        public override Dictionary<string, object?> ToDictionary()
        {
            var dict = new Dictionary<string, object?>
            {
                { "type", "developer_content" },
                { "instructions", Instructions },
                { "tools", Tools }
            };
            return dict.Where(kv => kv.Value != null).ToDictionary(kv => kv.Key, kv => kv.Value);
        }
    }

    /// <summary>
    /// Custom converter to allow the `content` field to accept both string and list of Content, matching flexibility in Python.
    /// </summary>
    public class ContentListOrStringConverter : JsonConverter<List<Content>>
    {
        public override List<Content> Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        {
            // The Rust side serializes *single* text contents as a plain string for convenience.
            // Detect this shortcut and normalize to a list.
            if (reader.TokenType == JsonTokenType.String)
            {
                var txt = reader.GetString();
                return new List<Content> { new TextContent(txt) };
            }
            // If it's an array/list
            if (reader.TokenType == JsonTokenType.StartArray)
            {
                var result = JsonSerializer.Deserialize<List<Content>>(ref reader, options);
                return result ?? new List<Content>();
            }

            throw new JsonException();
        }

        public override void Write(Utf8JsonWriter writer, List<Content> value, JsonSerializerOptions options)
        {
            // Serialize always as a list for compatibility symmetry
            JsonSerializer.Serialize(writer, value, options);
        }
    }

    // Message & Conversation -----------------------------------------------------

    public class Message
    {
        [JsonPropertyName("role")]
        [JsonConverter(typeof(LowercaseEnumConverter<Role>))]
        public Role Role { get; set; } = new();

        [JsonPropertyName("name")]
        public string? Name { get; set; }

        [JsonPropertyName("content")]
        [JsonConverter(typeof(ContentListOrStringConverter))]
        public List<Content> Content { get; set; } = new();

        [JsonPropertyName("channel")]
        public string? Channel { get; set; }

        [JsonPropertyName("recipient")]
        public string? Recipient { get; set; }

        [JsonPropertyName("content_type")]
        public string? ContentType { get; set; }

        // ------------------------------------------------------------------
        // Convenience constructors (mirroring the Rust API)
        // ------------------------------------------------------------------
        public static Message FromAuthorAndContent(Author author, object content)
        {
            if (content is string s)
                content = new TextContent(s);
            return new Message { Role = author.Role, Name = author.Name, Content = new() { (Content)content } };
        }
        public static Message FromRoleAndContent(Role role, object content) => FromAuthorAndContent(new Author(role), content);
        public static Message FromRoleAndContents(Role role, IEnumerable<Content> contents) => new() { Role = role, Name = null, Content = new(contents) };
    }

    public class Conversation
    {
        [JsonPropertyName("messages")]
        public List<Message> Messages { get; set; } = new();
        public static Conversation FromMessages(IEnumerable<Message> messages) => new() { Messages = new(messages) };
    }

    // Further Chat-related (tool/namespace/config) types...

    public record ToolDescription(string Name, string Description, Dictionary<string, object>? Parameters = null);

    // Used in SystemContent and elsewhere.
    [JsonConverter(typeof(JsonStringEnumConverter))]
    public enum ReasoningEffort
    {
        [JsonPropertyName("low")]
        Low,
        [JsonPropertyName("medium")]
        Medium,
        [JsonPropertyName("high")]
        High
    }

    public class ChannelConfig
    {
        [JsonPropertyName("valid_channels")]
        public List<string> ValidChannels { get; set; } = new();
        [JsonPropertyName("channel_required")]
        public bool ChannelRequired { get; set; }

        public ChannelConfig() { }
        public ChannelConfig(List<string> validChannels, bool channelRequired)
        {
            ValidChannels = validChannels;
            ChannelRequired = channelRequired;
        }

        /// <summary>
        /// Helper to require channels as in Python's ChannelConfig.require_channels
        /// </summary>
        public static ChannelConfig RequireChannels(List<string> channels) => new ChannelConfig(channels, true);
    }

    public record ToolNamespaceConfig(string Name, string? Description, List<ToolDescription> Tools)
    {
        // Python staticmethod browser()/python() mirror
        public static ToolNamespaceConfig Browser()
            => JsonSerializer.Deserialize<ToolNamespaceConfig>(HarmonyUtil.GetToolNamespaceConfig("browser"))!;
        public static ToolNamespaceConfig Python()
            => JsonSerializer.Deserialize<ToolNamespaceConfig>(HarmonyUtil.GetToolNamespaceConfig("python"))!;
    }

    // ---------------------------------------------------------------------------
    // Encoding interaction (thin wrappers around the Rust bindings)
    // ---------------------------------------------------------------------------

    public static class HarmonyEncodingName
    {
        public static string HarmonyGptOss => "HarmonyGptOss";
    }

    public class RenderConversationConfig
    {
        [JsonPropertyName("auto_drop_analysis")]
        public bool AutoDropAnalysis { get; set; } = true;
    }
    public class RenderOptions
    {
        [JsonPropertyName("conversation_has_function_tools")]
        public bool ConversationHasFunctionTools { get; set; } = false;
    }

    /// <summary>
    /// High-level wrapper around the Rust `PyHarmonyEncoding` class
    /// </summary>
    public class HarmonyEncoding : IDisposable
    {
        private IntPtr _handle;
        public IntPtr Handle => _handle;
        private static readonly JsonSerializerOptions _jsonOptions = new()
        {
            Converters =
            {

            },
            PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
            TypeInfoResolver = new DefaultJsonTypeInfoResolver()
        };

        public string Name
        {
            get
            {
                unsafe
                {
                    var ptr = HarmonyBindings.harmony_encoding_name(_handle.ToPointer());
                    return HarmonyUtil.ConsumeUtf8AndFree(ptr);
                }
            }
        }

        public HarmonyEncoding(string name)
        {
            unsafe
            {
                var namePtr = HarmonyUtil.StringToUtf8Ptr(name);
                _handle = (IntPtr)HarmonyBindings.harmony_encoding_new(namePtr);
                HarmonyUtil.FreeUtf8(namePtr);
            }
        }

        public void Dispose()
        {
            unsafe
            {
                HarmonyBindings.harmony_encoding_free(_handle.ToPointer());
            }
        }

        public HashSet<string> SpecialTokensSet
        {
            get
            {
                unsafe
                {
                    var ptr = HarmonyBindings.harmony_special_tokens(_handle.ToPointer());
                    var json = HarmonyUtil.ConsumeUtf8AndFree(ptr);
                    var tokens = JsonSerializer.Deserialize<string[]>(json);
                    return tokens == null ? new() : new(tokens);
                }
            }
        }

        // -- Rendering -----------------------------------------------------
        // Render a conversation for completion. Args are Conversation object, Role for the next turn, and RenderConversationConfig
        public List<int> RenderConversationForCompletion(Conversation conversation, Role nextTurnRole, RenderConversationConfig? config = null)
        {
            unsafe
            {
                var convJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(conversation, _jsonOptions));
                var roleStr = HarmonyUtil.StringToUtf8Ptr(nextTurnRole.ToString().ToLowerInvariant());
                var configJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(config ?? new RenderConversationConfig(), _jsonOptions));
                var ptr = HarmonyBindings.harmony_render_conversation_for_completion(_handle.ToPointer(), convJson, roleStr, configJson);

                if (ptr == null)
                {
                    unsafe
                    {
                        var lastErrPtr = HarmonyBindings.harmony_get_last_error();
                        throw new Exception(HarmonyUtil.ConsumeUtf8AndFree(lastErrPtr));
                    }
                }

                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(convJson); HarmonyUtil.FreeUtf8(roleStr); HarmonyUtil.FreeUtf8(configJson);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
        // Render a conversation without appending a new role.
        public List<int> RenderConversation(Conversation conversation, RenderConversationConfig? config = null)
        {
            unsafe
            {
                var convJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(conversation, _jsonOptions));
                var configJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(config ?? new RenderConversationConfig(), _jsonOptions));
                var ptr = HarmonyBindings.harmony_render_conversation(_handle.ToPointer(), convJson, configJson);

                if (ptr == null)
                {
                    unsafe
                    {
                        var lastErrPtr = HarmonyBindings.harmony_get_last_error();
                        throw new Exception(HarmonyUtil.ConsumeUtf8AndFree(lastErrPtr));
                    }
                }

                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(convJson); HarmonyUtil.FreeUtf8(configJson);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
        // Render a conversation for training.
        public List<int> RenderConversationForTraining(Conversation conversation, RenderConversationConfig? config = null)
        {
            unsafe
            {
                var convJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(conversation, _jsonOptions));
                var configJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(config ?? new RenderConversationConfig(), _jsonOptions));
                var ptr = HarmonyBindings.harmony_render_conversation_for_training(_handle.ToPointer(), convJson, configJson);

                if (ptr == null)
                {
                    unsafe
                    {
                        var lastErrPtr = HarmonyBindings.harmony_get_last_error();
                        throw new Exception(HarmonyUtil.ConsumeUtf8AndFree(lastErrPtr));
                    }
                }

                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(convJson); HarmonyUtil.FreeUtf8(configJson);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
        // Render a single message into tokens.
        public List<int> Render(Message message, RenderOptions? renderOptions = null)
        {
            unsafe
            {
                var msgJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(message, _jsonOptions));
                var renderOptJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(renderOptions ?? new RenderOptions(), _jsonOptions));
                var ptr = HarmonyBindings.harmony_render(_handle.ToPointer(), msgJson, renderOptJson);

                if (ptr == null)
                {
                    unsafe
                    {
                        var lastErrPtr = HarmonyBindings.harmony_get_last_error();
                        throw new Exception(HarmonyUtil.ConsumeUtf8AndFree(lastErrPtr));
                    }
                }

                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(msgJson); HarmonyUtil.FreeUtf8(renderOptJson);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }

        // -- Parsing -------------------------------------------------------
        // Incremental parsing, converting completion tokens to messages.
        public List<Message> ParseMessagesFromCompletionTokens(List<int> tokens, Role? role = null)
        {
            unsafe
            {
                var tokensJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(tokens, _jsonOptions));
                var rolePtr = role.HasValue ?
                    HarmonyUtil.StringToUtf8Ptr(role.Value.ToString().ToLowerInvariant())
                    : null;
                var ptr = HarmonyBindings.harmony_parse_messages_from_completion_tokens(_handle.ToPointer(), tokensJson, rolePtr);

                if (ptr == null)
                {
                    unsafe
                    {
                        var lastErrPtr = HarmonyBindings.harmony_get_last_error();
                        throw new Exception(HarmonyUtil.ConsumeUtf8AndFree(lastErrPtr));
                    }
                }

                var resultJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(tokensJson); if (rolePtr != null) HarmonyUtil.FreeUtf8(rolePtr);

                return JsonSerializer.Deserialize<List<Message>>(resultJson, _jsonOptions) ?? new();
            }
        }

        // -- Token decoding ------------------------------------------------
        // Decode a list of tokens into a UTF-8 string.
        public string DecodeUtf8(List<int> tokens)
        {
            unsafe
            {
                var tokensJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(tokens, _jsonOptions));
                var ptr = HarmonyBindings.harmony_decode_utf8(_handle.ToPointer(), tokensJson);
                var result = HarmonyUtil.ConsumeUtf8AndFree(ptr);
                HarmonyUtil.FreeUtf8(tokensJson);
                return result;
            }
        }
        // Raw decode, bytes
        public List<byte> DecodeBytes(List<int> tokens)
        {
            unsafe
            {
                var tokensJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(tokens, _jsonOptions));
                var ptr = HarmonyBindings.harmony_decode_bytes(_handle.ToPointer(), tokensJson);
                var result = HarmonyUtil.ConsumeUtf8AndFree(ptr);
                HarmonyUtil.FreeUtf8(tokensJson);
                return JsonSerializer.Deserialize<List<byte>>(result) ?? new();
            }
        }
        // Decode tokens to a string; WARNING: lossy by default, mirroring Python docstring.
        public string Decode(List<int> tokens, string errors = "replace")
        {
            var bytes = DecodeBytes(tokens);
            return Encoding.UTF8.GetString(bytes.ToArray());
        }
        // Encodes a string into tokens, as in the Python docstring.
        public List<int> Encode(string text, HashSet<string>? allowedSpecial = null)
        {
            unsafe
            {
                var textPtr = HarmonyUtil.StringToUtf8Ptr(text);
                var allowedJson = HarmonyUtil.StringToUtf8Ptr(JsonSerializer.Serialize(allowedSpecial ?? new HashSet<string>(), _jsonOptions));
                var ptr = HarmonyBindings.harmony_encode(_handle.ToPointer(), textPtr, allowedJson);
                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);

                HarmonyUtil.FreeUtf8(textPtr); HarmonyUtil.FreeUtf8(allowedJson);

                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
        // Returns if an individual token is a special token (mirrors the Python property).
        public bool IsSpecialToken(int token)
        {
            unsafe
            {
                return HarmonyBindings.harmony_is_special_token(_handle.ToPointer(), (uint)token) != 0;
            }
        }
        // -- Stop tokens --------------------------------------------------
        public List<int> StopTokens()
        {
            unsafe
            {
                var ptr = HarmonyBindings.harmony_stop_tokens(_handle.ToPointer());
                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
        public List<int> StopTokensForAssistantActions()
        {
            unsafe
            {
                var ptr = HarmonyBindings.harmony_stop_tokens_for_assistant_actions(_handle.ToPointer());
                var tokensJson = HarmonyUtil.ConsumeUtf8AndFree(ptr);
                return JsonSerializer.Deserialize<List<int>>(tokensJson) ?? new();
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Utility (String marshal, JSON, etc.): see Python wrapper helper re-exports
    // ---------------------------------------------------------------------------

    public static class HarmonyUtil
    {
        public static unsafe string ConsumeUtf8AndFree(byte* ptr)
        {
            if (ptr == null) return string.Empty;
            try
            {
                var list = new List<byte>();
                for (var p = ptr; *p != 0; p++) list.Add(*p);
                var s = Encoding.UTF8.GetString(list.ToArray());
                HarmonyBindings.harmony_free_string(ptr);
                return s;
            }
            catch
            {
                HarmonyBindings.harmony_free_string(ptr);
                throw;
            }
        }

        public static unsafe byte* StringToUtf8Ptr(string s)
        {
            if (string.IsNullOrEmpty(s)) return null;
            var bytes = Encoding.UTF8.GetBytes(s + '\0');
            var ptr = (byte*)Marshal.AllocHGlobal(bytes.Length);
            Marshal.Copy(bytes, 0, (IntPtr)ptr, bytes.Length);
            return ptr;
        }
        public static unsafe void FreeUtf8(byte* ptr)
        {
            if (ptr != null) Marshal.FreeHGlobal((IntPtr)ptr);
        }
        // Mirror the staticmethods ToolNamespaceConfig.browser/python in Python
        public static string GetToolNamespaceConfig(string name)
        {
            unsafe
            {
                var namePtr = StringToUtf8Ptr(name);
                var ptr = HarmonyBindings.harmony_get_tool_namespace_config(namePtr);
                var json = ConsumeUtf8AndFree(ptr);
                FreeUtf8(namePtr);
                return json;
            }
        }
    }

    // ---------------------------------------------------------------------------
    // HarmonyError Support for Exception interoperability
    // ---------------------------------------------------------------------------
    public class HarmonyError : Exception
    {
        public HarmonyError(string msg) : base(msg) { }
    }
}