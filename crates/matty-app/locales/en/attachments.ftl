## File uploads and downloads (attachments.rs)

# A file's kind when its name has no extension.
attachments-file = File
attachments-queued = Queued
attachments-too-big-one = { $name } is { $size }. Files must be under { $limit }.
attachments-too-big-many = { $count } files were not added. Files must be under { $limit }.
attachments-too-many-one = { $name } was not added: a message can carry { $max } files.
attachments-too-many-many = { $count } files were not added: a message can carry { $max } files.
attachments-upload-failed = Not sent: an upload failed. Retry it or remove it, then send.
attachments-downloading-percent = Downloading… { $percent }%
attachments-downloading-size = Downloading… { $size }
attachments-downloaded = Downloaded · { $meta }
attachments-download-failed = Download failed · Click to retry
attachments-waiting = Waiting for uploads
attachments-sends-when-done = Sends when the uploads finish…
attachments-queued-offline = Queued: files upload when you're back online. You can send now.
# The drop overlay: a channel by its handle, or a conversation by its name.
attachments-drop-channel = Drop to upload to #{ $channel }
attachments-drop-conversation = Drop to upload to { $channel }
