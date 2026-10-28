#!/usr/bin/env ruby
# frozen_string_literal: true

# Every external action must be pinned to a full commit SHA followed by a
# `# vX.Y.Z` comment (which Dependabot keeps in step with the SHA), and every
# container image to a sha256 digest.

require "yaml"

ROOT = File.expand_path("../..", __dir__)
PINNED_ACTION = %r{\A[\w.-]+/[\w.-]+(?:/[\w.-]+)*@[0-9a-f]{40}\z}
PINNED_IMAGE = /\A\S+@sha256:[0-9a-f]{64}\z/
VERSION_COMMENT = /\Av\d+(?:\.\d+)*\z/
USES_LINE = /\A\s*(?:-\s+)?uses:\s*(?<ref>[^\s#]+)\s*(?:#\s*(?<version>\S+))?\s*\z/

def collect(node, key, found = [])
  case node
  when Hash
    node.each do |name, value|
      found << value if name == key
      collect(value, key, found)
    end
  when Array
    node.each { |item| collect(item, key, found) }
  end
  found
end

errors = []
workflows = (Dir[File.join(ROOT, ".github/workflows/*.{yml,yaml}")] +
             Dir[File.join(ROOT, ".github/actions/**/action.{yml,yaml}")]).sort
errors << "no workflows found" if workflows.empty?

workflows.each do |path|
  name = path.delete_prefix("#{ROOT}/")
  source = File.read(path, encoding: "UTF-8")
  document = YAML.safe_load(source, aliases: false)

  pinned_lines = 0
  source.each_line.with_index(1) do |line, number|
    next unless line.match?(/\A\s*(?:-\s+)?uses:/)

    pinned_lines += 1
    match = USES_LINE.match(line.chomp)
    ref = match && match[:ref]
    next if ref&.start_with?("./")

    if ref.nil? || !PINNED_ACTION.match?(ref)
      errors << "#{name}:#{number}: action is not pinned to a full commit SHA: #{line.strip}"
    elsif !VERSION_COMMENT.match?(match[:version].to_s)
      errors << "#{name}:#{number}: pinned action needs a `# vX.Y.Z` comment: #{line.strip}"
    end
  end

  uses = collect(document, "uses")
  if uses.length != pinned_lines
    errors << "#{name}: found #{uses.length} `uses:` values but #{pinned_lines} block-style `uses:` lines"
  end
  uses.grep(%r{\Adocker://}).each do |ref|
    errors << "#{name}: docker action is not pinned to a digest: #{ref}" unless PINNED_IMAGE.match?(ref)
  end

  jobs = document.fetch("jobs", {})
  images = collect(jobs.values.map { |job| job.slice("container", "services") }, "image")
  images += jobs.values.map { |job| job["container"] }.grep(String)
  images.each do |image|
    errors << "#{name}: container image is not pinned to a digest: #{image}" unless PINNED_IMAGE.match?(image.to_s)
  end
end

if errors.empty?
  puts "action pins: #{workflows.length} workflow/action definitions verified"
else
  warn errors
  exit 1
end
