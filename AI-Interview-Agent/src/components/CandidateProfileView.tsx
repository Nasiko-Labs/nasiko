import React, { useState } from 'react';
import {
  CheckCircle2,
  Briefcase,
  GraduationCap,
  Code2,
  FolderGit2,
  Play,
  Clock,
  Sparkles,
  Award,
} from 'lucide-react';
import { CandidateProfile } from '../types';

interface CandidateProfileViewProps {
  profile: CandidateProfile;
  onStartInterview: (config: {
    role: string;
    difficulty: 'easy' | 'medium' | 'hard';
    number_of_questions: number;
    duration_minutes: number;
  }) => Promise<void>;
  startingInterview: boolean;
  onReupload: () => void;
}

export const CandidateProfileView: React.FC<CandidateProfileViewProps> = ({
  profile,
  onStartInterview,
  startingInterview,
  onReupload,
}) => {
  const [role, setRole] = useState(
    profile.experience?.[0]?.role || profile.seniority_estimate || 'Java Backend Developer'
  );
  const [difficulty, setDifficulty] = useState<'easy' | 'medium' | 'hard'>('medium');
  const [numQuestions, setNumQuestions] = useState(5);
  const [duration, setDuration] = useState(20);

  const skills = profile?.skills || {
    languages: [],
    backend: [],
    databases: [],
    cloud: [],
    devops: [],
    testing: [],
    frontend: [],
    other: [],
  };

  const skillsCategories = [
    { label: 'Languages', items: skills.languages || [], color: 'text-sky-400 bg-sky-950/40 border-sky-800' },
    { label: 'Backend Frameworks', items: skills.backend || [], color: 'text-indigo-400 bg-indigo-950/40 border-indigo-800' },
    { label: 'Databases & Cache', items: skills.databases || [], color: 'text-amber-400 bg-amber-950/40 border-amber-800' },
    { label: 'Cloud & Infrastructure', items: skills.cloud || [], color: 'text-cyan-400 bg-cyan-950/40 border-cyan-800' },
    { label: 'DevOps & Tooling', items: skills.devops || [], color: 'text-emerald-400 bg-emerald-950/40 border-emerald-800' },
    { label: 'Testing & QA', items: skills.testing || [], color: 'text-purple-400 bg-purple-950/40 border-purple-800' },
    { label: 'Frontend', items: skills.frontend || [], color: 'text-pink-400 bg-pink-950/40 border-pink-800' },
    { label: 'Concepts & Architectures', items: skills.other || [], color: 'text-slate-300 bg-slate-800 border-slate-700' },
  ].filter((cat) => cat.items && cat.items.length > 0);

  const handleLaunch = () => {
    onStartInterview({
      role,
      difficulty,
      number_of_questions: numQuestions,
      duration_minutes: duration,
    });
  };

  return (
    <div className="space-y-8 max-w-6xl mx-auto">
      {/* Header Banner */}
      <div className="bg-gradient-to-r from-slate-900 via-indigo-950/30 to-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 flex flex-col md:flex-row items-start md:items-center justify-between gap-6 shadow-xl">
        <div className="space-y-2">
          <div className="flex items-center space-x-2">
            <span className="px-2.5 py-0.5 rounded-full text-[11px] font-semibold bg-emerald-950 text-emerald-400 border border-emerald-800 flex items-center space-x-1">
              <CheckCircle2 className="w-3 h-3" />
              <span>Agent 1 Verified Extraction</span>
            </span>
            <span className="text-xs text-slate-400">Seniority: {profile.seniority_estimate}</span>
          </div>

          <h1 className="text-2xl sm:text-3xl font-bold text-white">
            {profile.candidate.name || 'Candidate Profile'}
          </h1>
          <p className="text-xs sm:text-sm text-slate-300 flex flex-wrap gap-4">
            {profile.candidate.email && <span>📧 {profile.candidate.email}</span>}
            {profile.candidate.phone && <span>📞 {profile.candidate.phone}</span>}
            {profile.candidate.location && <span>📍 {profile.candidate.location}</span>}
          </p>
          <p className="text-xs sm:text-sm text-slate-400 max-w-3xl leading-relaxed pt-1">
            {profile.resume_summary}
          </p>
        </div>

        <button
          onClick={onReupload}
          className="px-4 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs font-medium text-slate-300 border border-slate-700 transition-colors"
        >
          Upload Different Resume
        </button>
      </div>

      {/* Main Grid: Left = Profile Details, Right = Interview Configuration */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-8">
        {/* Left Column: Skills, Projects, Experience */}
        <div className="lg:col-span-2 space-y-6">
          {/* Categorized Skills */}
          <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md">
            <div className="flex items-center space-x-2 mb-4">
              <Code2 className="w-4 h-4 text-indigo-400" />
              <h2 className="text-sm font-semibold text-white">Verified Technical Skills by Category</h2>
            </div>

            <div className="space-y-4">
              {skillsCategories.map((cat) => (
                <div key={cat.label} className="space-y-1.5">
                  <div className="text-[11px] font-medium text-slate-400">{cat.label}</div>
                  <div className="flex flex-wrap gap-1.5">
                    {cat.items.map((item) => (
                      <span
                        key={item}
                        className={`text-xs px-2.5 py-1 rounded-md border font-medium ${cat.color}`}
                      >
                        {item}
                      </span>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Key Projects & Extracted Interview Topics */}
          {profile.projects && profile.projects.length > 0 && (
            <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md">
              <div className="flex items-center space-x-2 mb-4">
                <FolderGit2 className="w-4 h-4 text-cyan-400" />
                <h2 className="text-sm font-semibold text-white">
                  Projects & Technical Concepts for Questioning
                </h2>
              </div>

              <div className="space-y-4">
                {profile.projects.map((proj, idx) => (
                  <div
                    key={idx}
                    className="border border-slate-800 bg-slate-950/40 rounded-xl p-4 space-y-2"
                  >
                    <div className="flex items-center justify-between">
                      <h3 className="text-sm font-semibold text-white">{proj.name}</h3>
                      <div className="flex flex-wrap gap-1">
                        {proj.technologies.slice(0, 3).map((t) => (
                          <span
                            key={t}
                            className="text-[10px] bg-slate-800 text-slate-300 px-2 py-0.5 rounded"
                          >
                            {t}
                          </span>
                        ))}
                      </div>
                    </div>
                    <p className="text-xs text-slate-400">{proj.description}</p>
                    {proj.candidate_contribution && (
                      <p className="text-xs text-slate-300">
                        <span className="font-semibold text-indigo-300">Contribution: </span>
                        {proj.candidate_contribution}
                      </p>
                    )}

                    {proj.technical_concepts && proj.technical_concepts.length > 0 && (
                      <div className="pt-2 border-t border-slate-800/80 flex flex-wrap gap-1.5 items-center">
                        <span className="text-[11px] text-slate-500 mr-1">Key Concepts:</span>
                        {proj.technical_concepts.map((c) => (
                          <span
                            key={c}
                            className="text-[10px] bg-indigo-950/50 text-indigo-300 border border-indigo-800 px-2 py-0.5 rounded"
                          >
                            {c}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Work Experience */}
          {profile.experience && profile.experience.length > 0 && (
            <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md">
              <div className="flex items-center space-x-2 mb-4">
                <Briefcase className="w-4 h-4 text-emerald-400" />
                <h2 className="text-sm font-semibold text-white">Experience Timeline</h2>
              </div>

              <div className="space-y-4">
                {profile.experience.map((exp, idx) => (
                  <div key={idx} className="border-l-2 border-slate-700 pl-4 space-y-1">
                    <div className="flex items-center justify-between">
                      <h3 className="text-sm font-semibold text-white">
                        {exp.role} · <span className="text-indigo-400 font-normal">{exp.company}</span>
                      </h3>
                      <span className="text-xs text-slate-500">{exp.duration}</span>
                    </div>
                    {exp.responsibilities && (
                      <ul className="text-xs text-slate-400 list-disc list-inside space-y-1 pt-1">
                        {exp.responsibilities.map((r, ri) => (
                          <li key={ri}>{r}</li>
                        ))}
                      </ul>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Education & Certifications */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {profile.education && profile.education.length > 0 && (
              <div className="bg-slate-900 border border-slate-800 rounded-2xl p-5">
                <div className="flex items-center space-x-2 mb-3">
                  <GraduationCap className="w-4 h-4 text-amber-400" />
                  <h3 className="text-xs font-semibold text-white">Education</h3>
                </div>
                {profile.education.map((edu, i) => (
                  <div key={i} className="text-xs space-y-0.5">
                    <div className="font-medium text-slate-200">{edu.degree}</div>
                    <div className="text-slate-400">{edu.university} ({edu.graduation_year})</div>
                    {edu.cgpa && <div className="text-slate-500">CGPA: {edu.cgpa}</div>}
                  </div>
                ))}
              </div>
            )}

            {profile.certifications && profile.certifications.length > 0 && (
              <div className="bg-slate-900 border border-slate-800 rounded-2xl p-5">
                <div className="flex items-center space-x-2 mb-3">
                  <Award className="w-4 h-4 text-purple-400" />
                  <h3 className="text-xs font-semibold text-white">Certifications</h3>
                </div>
                <div className="space-y-1.5">
                  {profile.certifications.map((cert, i) => (
                    <div key={i} className="text-xs text-slate-300 flex items-center space-x-1.5">
                      <span className="w-1.5 h-1.5 rounded-full bg-purple-400" />
                      <span>{cert}</span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Right Column: Launch Interview Configuration */}
        <div className="space-y-6">
          <div className="bg-slate-900 border border-indigo-900/60 rounded-2xl p-6 shadow-xl sticky top-20 space-y-5">
            <div className="flex items-center space-x-2 border-b border-slate-800 pb-3">
              <Sparkles className="w-4 h-4 text-indigo-400" />
              <div>
                <h2 className="text-sm font-semibold text-white">Interview Configuration</h2>
                <p className="text-[11px] text-slate-400">Calibrate Agent 2 (Interview Agent)</p>
              </div>
            </div>

            {/* Target Role */}
            <div className="space-y-1.5">
              <label className="text-xs font-medium text-slate-300 block">Target Role</label>
              <input
                type="text"
                value={role}
                onChange={(e) => setRole(e.target.value)}
                className="w-full bg-slate-950 border border-slate-800 rounded-lg px-3 py-2 text-xs text-white focus:outline-none focus:border-indigo-500"
                placeholder="e.g. Java Backend Developer"
              />
            </div>

            {/* Difficulty */}
            <div className="space-y-1.5">
              <label className="text-xs font-medium text-slate-300 block">Baseline Difficulty</label>
              <div className="grid grid-cols-3 gap-2">
                {(['easy', 'medium', 'hard'] as const).map((d) => (
                  <button
                    key={d}
                    type="button"
                    onClick={() => setDifficulty(d)}
                    className={`py-2 text-xs font-medium capitalize rounded-lg border transition-all ${
                      difficulty === d
                        ? 'bg-indigo-600 text-white border-indigo-500 shadow-sm'
                        : 'bg-slate-950 text-slate-400 border-slate-800 hover:text-slate-200'
                    }`}
                  >
                    {d}
                  </button>
                ))}
              </div>
              <p className="text-[10px] text-slate-500 mt-1">
                Note: Questions will adapt dynamically after each answer evaluation!
              </p>
            </div>

            {/* Number of Questions */}
            <div className="space-y-1.5">
              <label className="text-xs font-medium text-slate-300 block">
                Number of Questions ({numQuestions})
              </label>
              <div className="grid grid-cols-4 gap-2">
                {[3, 5, 8, 10].map((num) => (
                  <button
                    key={num}
                    type="button"
                    onClick={() => setNumQuestions(num)}
                    className={`py-1.5 text-xs font-medium rounded-lg border transition-all ${
                      numQuestions === num
                        ? 'bg-indigo-600 text-white border-indigo-500'
                        : 'bg-slate-950 text-slate-400 border-slate-800 hover:text-slate-200'
                    }`}
                  >
                    {num} Qs
                  </button>
                ))}
              </div>
            </div>

            {/* Estimated Duration */}
            <div className="flex items-center justify-between text-xs text-slate-400 bg-slate-950/60 p-3 rounded-lg border border-slate-800">
              <div className="flex items-center space-x-2">
                <Clock className="w-3.5 h-3.5 text-slate-500" />
                <span>Target Session Duration</span>
              </div>
              <span className="font-mono text-white font-medium">{duration} mins</span>
            </div>

            {/* Launch Button */}
            <button
              onClick={handleLaunch}
              disabled={startingInterview || !role}
              className="w-full py-3.5 px-4 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 disabled:opacity-50 text-white font-medium text-sm flex items-center justify-center space-x-2 shadow-lg shadow-indigo-500/25 transition-all"
            >
              {startingInterview ? (
                <>
                  <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  <span>Nasiko Orchestrator Booting Agent 2...</span>
                </>
              ) : (
                <>
                  <Play className="w-4 h-4 fill-current" />
                  <span>Start Adaptive Interview</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
