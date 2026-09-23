<!--
  The app's one search box: a magnifier and a field, wherever something is
  searchable. It fills the slot it is handed and lets the container decide the
  width, so the same control serves a dialog header, a panel header and a
  popup without any of them restating what a search box looks like.

  A surface that isn't the usual panel ground tunes the field into itself with
  `--search-field-bg` and `--search-field-font-size`, set on any ancestor.
-->
<script lang="ts">
    import Icon from '../icons/Icon.svelte';

    interface Props {
        /** The query. */
        value?: string;
        /** Also the field's accessible name: the magnifier carries no text. */
        placeholder?: string;
        /** The input itself, for callers that focus it when their dialog opens
         *  or test an event's target against it. */
        element?: HTMLInputElement | null;
        onkeydown?: (e: KeyboardEvent) => void;
        autofocus?: boolean;
    }

    let {
        value = $bindable(''),
        placeholder = 'Search…',
        element = $bindable(null),
        onkeydown,
        autofocus = false,
    }: Props = $props();
</script>

<div class="search-field">
    <Icon name="fa6-solid:magnifying-glass" />
    <!-- svelte-ignore a11y_autofocus -->
    <input
        bind:this={element}
        bind:value
        type="search"
        {placeholder}
        {onkeydown}
        {autofocus}
        aria-label={placeholder}
        autocomplete="off"
        spellcheck="false"
    />
</div>

<style>
    .search-field {
        display: flex;
        align-items: center;
        gap: 8px;
        /* Takes the room it is given; a caller wanting a narrower box says so
         * from the outside rather than from in here. */
        flex: 1 1 auto;
        min-width: 0;
        box-sizing: border-box;
        background: var(--search-field-bg, var(--bg-hover));
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        padding: 5px 10px;
        /* The magnifier reads as chrome; the typed query is full-strength. */
        color: var(--text-muted);
        font-size: var(--search-field-font-size, 13px);
    }

    .search-field:focus-within {
        border-color: var(--accent);
    }

    .search-field input {
        flex: 1;
        min-width: 0;
        background: transparent;
        border: none;
        color: var(--text);
        font-family: inherit;
        font-size: inherit;
        outline: none;
    }
</style>
